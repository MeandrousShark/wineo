use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, IoContext, Result};

/// Where wineo keeps its files, following the XDG base directory spec.
#[derive(Debug, Clone)]
pub struct Paths {
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Resolves paths from an arbitrary variable source, so tests don't have
    /// to mutate the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let home = lookup("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(Error::NoHome)?;
        // The spec says relative values are invalid and must be ignored.
        let base = |var: &str, fallback: &str| {
            lookup(var)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
                .join("wineo")
        };
        Ok(Self {
            config: base("XDG_CONFIG_HOME", ".config"),
            data: base("XDG_DATA_HOME", ".local/share"),
            state: base("XDG_STATE_HOME", ".local/state"),
            home,
        })
    }

    pub fn global_config(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn environments_dir(&self) -> PathBuf {
        self.config.join("environments")
    }

    /// A template is configuration, not data, so it sits beside the configs
    /// rather than beside the builds it names.
    pub fn templates_dir(&self) -> PathBuf {
        self.config.join("templates")
    }

    pub fn environment_config(&self, name: &str) -> PathBuf {
        self.environments_dir().join(format!("{name}.toml"))
    }

    /// The XDG applications directory. Desktop entries have to sit in the
    /// standard location, not under wineo's own data dir, or nothing reads them.
    pub fn applications_dir(&self) -> PathBuf {
        self.xdg_data().join("applications")
    }

    /// Where wine is pointed while an icon is extracted, so the icon theme it
    /// insists on writing never lands in the real one.
    pub fn icon_scratch_dir(&self) -> PathBuf {
        self.state.join(".icons")
    }

    /// The XDG icon theme root, for the same reason.
    pub fn icons_dir(&self) -> PathBuf {
        self.xdg_data().join("icons")
    }

    /// `$XDG_DATA_HOME` itself, which every wineo data path hangs off.
    fn xdg_data(&self) -> &Path {
        self.data
            .parent()
            .expect("the data dir is always `<xdg data home>/wineo`")
    }

    pub fn default_prefix_root(&self) -> PathBuf {
        self.data.join("prefixes")
    }

    pub fn default_runner_root(&self) -> PathBuf {
        self.data.join("runners")
    }

    pub fn default_dxvk_root(&self) -> PathBuf {
        self.data.join("dxvk")
    }

    pub fn default_vkd3d_root(&self) -> PathBuf {
        self.data.join("vkd3d")
    }

    /// Operation lock files. Dotted, so it can never collide with the runtime
    /// dir of an environment, and stable, so removing one leaves it alone.
    pub fn locks_dir(&self) -> PathBuf {
        self.state.join(".locks")
    }

    pub fn runtime_dir(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }

    /// Where a Heroic import appends its `-v` log, one file per environment name.
    pub fn import_log(&self, name: &str) -> PathBuf {
        self.state.join("imports").join(format!("{name}.log"))
    }

    /// Expands a leading `~` in a path taken from a config file or the command line.
    pub fn expand(&self, path: &str) -> PathBuf {
        if path == "~" {
            return self.home.clone();
        }
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => PathBuf::from(path),
        }
    }

    /// Expands `~` and resolves relative paths against the current directory.
    pub fn absolute(&self, path: &str) -> Result<PathBuf> {
        std::path::absolute(self.expand(path)).context(|| format!("failed to resolve {path}"))
    }

    /// Turns a path back into its config form, writing paths under the home
    /// directory as `~/...` so configs survive a move to another machine.
    pub fn contract(&self, path: &Path) -> Result<String> {
        let text = |path: &Path| {
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::NonUtf8Path(path.to_owned()))
        };
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => Ok("~".to_owned()),
            Ok(rest) => Ok(format!("~/{}", text(rest)?)),
            Err(_) => text(path),
        }
    }
}

/// Canonicalizes as much of `path` as exists, so two spellings of the same
/// directory compare equal even before it is created.
pub fn resolve(path: &Path) -> PathBuf {
    let components: Vec<Component> = path.components().collect();
    for split in (0..=components.len()).rev() {
        let head: PathBuf = components[..split].iter().collect();
        let Ok(base) = std::fs::canonicalize(&head) else {
            continue;
        };
        // Nothing past the longest existing ancestor exists, so none of it can
        // be a symlink and `..` is safe to fold in without touching the disk.
        let mut resolved = base;
        for component in &components[split..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    resolved.pop();
                }
                other => resolved.push(other),
            }
        }
        return resolved;
    }
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn paths(vars: &[(&str, &str)]) -> Result<Paths> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(key, value)| (key.to_string(), OsString::from(value)))
            .collect();
        Paths::from_lookup(|key| vars.get(key).cloned())
    }

    #[test]
    fn defaults_follow_xdg_fallbacks() {
        let paths = paths(&[("HOME", "/home/me")]).unwrap();
        assert_eq!(
            paths.global_config(),
            Path::new("/home/me/.config/wineo/config.toml")
        );
        assert_eq!(
            paths.environment_config("notepad"),
            Path::new("/home/me/.config/wineo/environments/notepad.toml")
        );
        assert_eq!(
            paths.default_prefix_root(),
            Path::new("/home/me/.local/share/wineo/prefixes")
        );
        assert_eq!(
            paths.runtime_dir("notepad"),
            Path::new("/home/me/.local/state/wineo/notepad")
        );
        assert_eq!(
            paths.import_log("palworld"),
            Path::new("/home/me/.local/state/wineo/imports/palworld.log")
        );
    }

    #[test]
    fn xdg_variables_override_defaults() {
        let paths = paths(&[
            ("HOME", "/home/me"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_STATE_HOME", "/state"),
        ])
        .unwrap();
        assert_eq!(paths.global_config(), Path::new("/cfg/wineo/config.toml"));
        assert_eq!(
            paths.default_prefix_root(),
            Path::new("/data/wineo/prefixes")
        );
        assert_eq!(paths.runtime_dir("x"), Path::new("/state/wineo/x"));
    }

    #[test]
    fn relative_and_empty_xdg_values_are_ignored() {
        let paths = paths(&[
            ("HOME", "/home/me"),
            ("XDG_CONFIG_HOME", "relative"),
            ("XDG_STATE_HOME", ""),
        ])
        .unwrap();
        assert_eq!(
            paths.global_config(),
            Path::new("/home/me/.config/wineo/config.toml")
        );
        assert_eq!(
            paths.runtime_dir("x"),
            Path::new("/home/me/.local/state/wineo/x")
        );
    }

    #[test]
    fn missing_home_is_an_error() {
        assert!(matches!(paths(&[]), Err(Error::NoHome)));
        assert!(matches!(paths(&[("HOME", "rel")]), Err(Error::NoHome)));
    }

    #[test]
    fn expand_and_contract_round_trip() {
        let paths = paths(&[("HOME", "/home/me")]).unwrap();
        assert_eq!(paths.expand("~"), Path::new("/home/me"));
        assert_eq!(paths.expand("~/Apps/x"), Path::new("/home/me/Apps/x"));
        assert_eq!(paths.expand("/opt/x"), Path::new("/opt/x"));
        assert_eq!(paths.expand("~other/x"), Path::new("~other/x"));

        assert_eq!(paths.contract(Path::new("/home/me")).unwrap(), "~");
        assert_eq!(
            paths.contract(Path::new("/home/me/Apps/x")).unwrap(),
            "~/Apps/x"
        );
        assert_eq!(paths.contract(Path::new("/opt/x")).unwrap(), "/opt/x");
        assert_eq!(
            paths.contract(Path::new("/home/meow/x")).unwrap(),
            "/home/meow/x"
        );
    }

    #[test]
    fn absolute_resolves_relative_paths() {
        let paths = paths(&[("HOME", "/home/me")]).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(paths.absolute("some/dir").unwrap(), cwd.join("some/dir"));
        assert_eq!(paths.absolute("~/x").unwrap(), Path::new("/home/me/x"));
    }
}
