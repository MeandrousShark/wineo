//! Named presets `wineo new` starts an environment from.
//!
//! A template only sets config values. `prefix` and `targets` are per
//! environment by definition, so a template may not carry them.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, value};

use crate::config::{self, Arch, GamescopeSetting, HooksSetting};
use crate::error::{Error, Result};
use crate::files;
use crate::name;
use crate::paths::Paths;
use crate::runner::Runner;
use crate::runners::Runners;

/// Shipped templates, used when no file of the same name is installed. `latest`
/// and `proton` are what let one exist at all: naming a version nobody has
/// would make it fail on every fresh machine. With a Proton build installed,
/// gaming runs on it and its DXVK and vkd3d picks drop out.
const BUILT_IN: [(&str, &str); 1] = [(
    "gaming",
    "schema_version = 1\ntags = [\"game\"]\nrunner = \"proton\"\ndxvk = \"latest\"\nvkd3d = \"latest\"\ngamemode = true\n",
)];

/// What a template's `runner` can be besides a runner's name: the newest
/// Proton build installed, or the default runner when there is none.
pub const NEWEST_PROTON: &str = "proton";

/// Everything a template may set.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub winetricks: Vec<String>,
    pub runner: Option<String>,
    pub arch: Option<Arch>,
    pub gamemode: Option<bool>,
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    pub gamescope: Option<GamescopeSetting>,
    pub hooks: Option<HooksSetting>,
    pub gameid: Option<String>,
    pub store: Option<String>,
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Template {
    /// The runner this template asks for, by name, with `proton` resolved to
    /// the newest Proton build installed or `fallback` when there is none.
    /// None when it asks for no runner at all.
    pub fn runner_name(&self, runners: &Runners, fallback: &str) -> Result<Option<String>> {
        match self.runner.as_deref() {
            Some(NEWEST_PROTON) => Ok(Some(match runners.newest_proton()? {
                Some(runner) => runner.name().to_owned(),
                None => fallback.to_owned(),
            })),
            other => Ok(other.map(str::to_owned)),
        }
    }

    /// The template as it applies on `runner`. Proton bundles its own DXVK
    /// and vkd3d-proton, so a template's picks of those are left out for it.
    /// The flag says whether anything was.
    pub fn for_runner(&self, runner: &Runner) -> (Template, bool) {
        let mut fitted = self.clone();
        if runner.is_proton() && (fitted.dxvk.is_some() || fitted.vkd3d.is_some()) {
            fitted.dxvk = None;
            fitted.vkd3d = None;
            return (fitted, true);
        }
        (fitted, false)
    }

    /// Adds everything the template sets to a new environment's config.
    /// `runner` and `arch` are left out: the caller resolves those before the
    /// prefix is created, so they are already written.
    pub fn apply(&self, doc: &mut DocumentMut) {
        if !self.tags.is_empty() {
            let mut tags = Array::new();
            for tag in &self.tags {
                tags.push(tag.as_str());
            }
            doc["tags"] = value(tags);
        }
        if !self.winetricks.is_empty() {
            let mut verbs = Array::new();
            for verb in &self.winetricks {
                verbs.push(verb.as_str());
            }
            doc["winetricks"] = value(verbs);
        }
        for (key, set) in [
            ("gamemode", self.gamemode),
            ("wayland", self.wayland),
            ("mangohud", self.mangohud),
        ] {
            if let Some(set) = set {
                doc[key] = value(set);
            }
        }
        for (key, set) in [
            ("gameid", &self.gameid),
            ("store", &self.store),
            ("dxvk", &self.dxvk),
            ("vkd3d", &self.vkd3d),
        ] {
            if let Some(set) = set {
                doc[key] = value(set.as_str());
            }
        }
        for (key, set) in &self.env {
            doc["env"][key.as_str()] = value(set.as_str());
        }
        if let Some(gamescope) = &self.gamescope {
            doc["gamescope"] = gamescope.to_item();
        }
        if let Some(hooks) = &self.hooks {
            doc["hooks"] = hooks.to_item();
        }
    }
}

/// One row of `wineo templates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    /// False once a file of the same name shadows the shipped one.
    pub built_in: bool,
}

/// The directory a user's own templates live in.
#[derive(Debug, Clone)]
pub struct Templates {
    dir: PathBuf,
}

impl Templates {
    pub fn new(paths: &Paths) -> Self {
        Self::at(paths.templates_dir())
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn io_error(&self, source: io::Error) -> Error {
        Error::Io {
            context: format!("failed to read {}", self.dir.display()),
            source,
        }
    }

    /// The template `--template` names. A file wins over a built-in of the
    /// same name, so a shipped one can be replaced rather than worked around.
    pub fn resolve(&self, name: &str) -> Result<Template> {
        if !name::is_valid(name) {
            return Err(Error::InvalidName(name.to_owned()));
        }
        let path = self.dir.join(format!("{name}.toml"));
        let read = files::read_to_string_optional(&path).map_err(|err| Error::Io {
            context: format!("failed to read {}", path.display()),
            source: err,
        })?;
        if let Some(text) = read {
            return parse(&path, &text);
        }
        match BUILT_IN.iter().find(|(built_in, _)| *built_in == name) {
            Some((_, text)) => parse(&path, text),
            None => Err(Error::UnknownTemplate {
                name: name.to_owned(),
                available: self.list()?.into_iter().map(|row| row.name).collect(),
                dir: self.dir.clone(),
            }),
        }
    }

    /// Every template, built-in or installed, sorted by name.
    pub fn list(&self) -> Result<Vec<Listed>> {
        let mut rows: Vec<Listed> = BUILT_IN
            .iter()
            .map(|(name, _)| Listed {
                name: (*name).to_owned(),
                built_in: true,
            })
            .collect();

        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries.collect::<Vec<_>>(),
            // No directory yet just means nothing of the user's own.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(self.io_error(err)),
        };
        for entry in entries {
            let path = entry.map_err(|err| self.io_error(err))?.path();
            if path.extension() != Some("toml".as_ref()) {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !name::is_valid(name) {
                continue;
            }
            match rows.iter_mut().find(|row| row.name == name) {
                // A file of the same name is now what the name means.
                Some(row) => row.built_in = false,
                None => rows.push(Listed {
                    name: name.to_owned(),
                    built_in: false,
                }),
            }
        }
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }
}

fn parse(path: &Path, text: &str) -> Result<Template> {
    crate::config::check_schema(path, text)?;
    let mut table: toml::Table = toml::from_str(text).map_err(|source| Error::ConfigSyntax {
        path: path.to_owned(),
        source: Box::new(source),
    })?;
    // Both are per-environment by definition, so they are called out rather
    // than lumped in with an unknown key.
    for key in ["prefix", "targets"] {
        if table.contains_key(key) {
            return Err(Error::ConfigInvalid {
                path: path.to_owned(),
                message: format!("`{key}` belongs to an environment, not a template"),
            });
        }
    }
    table.remove("schema_version");
    let template: Template = table.try_into().map_err(|source| Error::ConfigSyntax {
        path: path.to_owned(),
        source: Box::new(source),
    })?;
    config::validate_tags(&template.tags).map_err(|message| Error::ConfigInvalid {
        path: path.to_owned(),
        message,
    })?;
    config::validate_winetricks(&template.winetricks).map_err(|message| Error::ConfigInvalid {
        path: path.to_owned(),
        message,
    })?;
    Ok(template)
}

#[cfg(test)]
mod tests;
