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

use crate::config::{self, Arch, GamescopeSetting};
use crate::error::{Error, Result};
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
    "schema_version = 1\nrunner = \"proton\"\ndxvk = \"latest\"\nvkd3d = \"latest\"\ngamemode = true\n",
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
    pub runner: Option<String>,
    pub arch: Option<Arch>,
    pub gamemode: Option<bool>,
    pub wayland: Option<bool>,
    pub mangohud: Option<bool>,
    pub gamescope: Option<GamescopeSetting>,
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
        match fs::read_to_string(&path) {
            Ok(text) => return parse(&path, &text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(Error::Io {
                    context: format!("failed to read {}", path.display()),
                    source: err,
                });
            }
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
    Ok(template)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates(root: &Path) -> Templates {
        Templates::at(root.join("templates"))
    }

    fn write(root: &Path, name: &str, text: &str) {
        let dir = root.join("templates");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    #[test]
    fn the_built_in_names_no_version() {
        let root = tempfile::tempdir().unwrap();
        let gaming = templates(root.path()).resolve("gaming").unwrap();
        assert_eq!(gaming.dxvk.as_deref(), Some("latest"));
        assert_eq!(gaming.vkd3d.as_deref(), Some("latest"));
        assert_eq!(gaming.gamemode, Some(true));
        assert_eq!(gaming.runner.as_deref(), Some(NEWEST_PROTON));
    }

    #[test]
    fn a_file_shadows_a_built_in_of_the_same_name() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "gaming", "gamemode = false\n");
        let gaming = templates(root.path()).resolve("gaming").unwrap();
        assert_eq!(gaming.gamemode, Some(false));
        assert_eq!(gaming.dxvk, None);

        let listed = templates(root.path()).list().unwrap();
        assert_eq!(
            listed,
            [Listed {
                name: "gaming".to_owned(),
                built_in: false
            }]
        );
    }

    #[test]
    fn listing_covers_built_ins_and_files() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            templates(root.path()).list().unwrap(),
            [Listed {
                name: "gaming".to_owned(),
                built_in: true
            }]
        );

        write(root.path(), "hades", "runner = \"wine-tkg\"\n");
        write(root.path(), "notes", "not a template");
        fs::write(root.path().join("templates/README"), "").unwrap();
        let listed = templates(root.path()).list().unwrap();
        let names: Vec<&str> = listed.iter().map(|row| row.name.as_str()).collect();
        // Listing does not parse, so a broken file still shows up.
        assert_eq!(names, ["gaming", "hades", "notes"]);
    }

    #[test]
    fn a_template_parses_everything_an_environment_may_set() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "full",
            "runner = \"wine-tkg\"\narch = \"win32\"\ngamemode = true\ndxvk = \"2.7\"\nvkd3d = \"2.14\"\n\n[env]\nDXVK_HUD = \"fps\"\n",
        );
        let full = templates(root.path()).resolve("full").unwrap();
        assert_eq!(full.runner.as_deref(), Some("wine-tkg"));
        assert_eq!(full.arch, Some(Arch::Win32));
        assert_eq!(full.dxvk.as_deref(), Some("2.7"));
        assert_eq!(full.env["DXVK_HUD"], "fps");
    }

    #[test]
    fn template_tags_are_validated_and_applied() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "good", "tags = ['game', 'co-op']\n");
        let template = templates(root.path()).resolve("good").unwrap();
        assert_eq!(template.tags, ["game", "co-op"]);
        let mut doc: DocumentMut = "kind = 'native'\n".parse().unwrap();
        template.apply(&mut doc);
        let config =
            config::EnvironmentConfig::parse(Path::new("/native.toml"), &doc.to_string()).unwrap();
        assert_eq!(config.tags, ["game", "co-op"]);

        for tags in ["['co op']", "['game', 'game']"] {
            write(root.path(), "bad", &format!("tags = {tags}\n"));
            assert!(matches!(
                templates(root.path()).resolve("bad"),
                Err(Error::ConfigInvalid { .. })
            ));
        }
    }

    #[test]
    fn per_environment_keys_are_refused_by_name() {
        let root = tempfile::tempdir().unwrap();
        for key in ["prefix = \"/p\"", "[targets.app]\nexe = \"a\""] {
            write(root.path(), "bad", key);
            let err = templates(root.path()).resolve("bad").unwrap_err();
            assert!(
                err.to_string().contains("belongs to an environment"),
                "{key}: {err}"
            );
        }
    }

    #[test]
    fn a_template_may_carry_the_schema_version() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "v1", "schema_version = 1\ngamemode = true\n");
        write(root.path(), "legacy", "gamemode = true\n");
        let v1 = templates(root.path()).resolve("v1").unwrap();
        assert_eq!(v1, templates(root.path()).resolve("legacy").unwrap());
        assert_eq!(v1.gamemode, Some(true));
    }

    #[test]
    fn reading_a_template_leaves_it_alone() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "legacy", "# mine\ngamemode = true\n");
        let path = root.path().join("templates/legacy.toml");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        templates(root.path()).resolve("legacy").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# mine\ngamemode = true\n"
        );
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn unsupported_template_versions_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        for value in ["0", "2", "\"1\""] {
            write(root.path(), "t", &format!("schema_version = {value}\n"));
            assert!(
                matches!(
                    templates(root.path()).resolve("t"),
                    Err(Error::SchemaVersion { .. })
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "typo", "gamemod = true\n");
        assert!(templates(root.path()).resolve("typo").is_err());
    }

    #[test]
    fn an_unknown_template_says_what_is_available() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "hades", "");
        let err = templates(root.path()).resolve("nope").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("gaming"), "{message}");
        assert!(message.contains("hades"), "{message}");

        assert!(matches!(
            templates(root.path()).resolve("bad name"),
            Err(Error::InvalidName(_))
        ));
    }

    #[test]
    fn applying_writes_only_what_the_template_sets() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "t",
            "runner = \"wine-tkg\"\narch = \"win32\"\ndxvk = \"latest\"\ngamemode = true\n\n[env]\nDXVK_HUD = \"fps\"\n",
        );
        let mut doc = DocumentMut::new();
        doc["prefix"] = value("~/p");
        templates(root.path()).resolve("t").unwrap().apply(&mut doc);
        let text = doc.to_string();

        assert!(text.contains("dxvk = \"latest\""), "{text}");
        assert!(text.contains("gamemode = true"), "{text}");
        assert!(text.contains("DXVK_HUD = \"fps\""), "{text}");
        // The caller already wrote these, having resolved them first.
        assert!(!text.contains("runner"), "{text}");
        assert!(!text.contains("arch"), "{text}");
        assert!(!text.contains("vkd3d"), "{text}");
    }

    #[test]
    fn launch_settings_carry_into_the_environment() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "t",
            "wayland = true\nmangohud = false\n\n[gamescope]\nwidth = 1920\nwindow = \"fullscreen\"\n",
        );
        let template = templates(root.path()).resolve("t").unwrap();
        assert_eq!(template.wayland, Some(true));
        assert_eq!(template.mangohud, Some(false));
        let mut doc = crate::config::new_document();
        doc["prefix"] = value("/p");
        doc["runner"] = value("wine");
        doc["arch"] = value("win64");
        template.apply(&mut doc);
        let text = doc.to_string();
        assert!(text.contains("wayland = true\n"), "{text}");
        assert!(text.contains("mangohud = false\n"), "{text}");
        assert!(
            text.contains("[gamescope]\nwidth = 1920\nwindow = \"fullscreen\"\n"),
            "{text}"
        );
        let config = crate::config::EnvironmentConfig::parse(Path::new("/t.toml"), &text).unwrap();
        assert_eq!(config.gamescope, template.gamescope);

        write(root.path(), "off", "gamescope = false\n");
        let off = templates(root.path()).resolve("off").unwrap();
        let mut doc = DocumentMut::new();
        off.apply(&mut doc);
        assert_eq!(doc.to_string(), "gamescope = false\n");

        write(root.path(), "on", "gamescope = true\n");
        assert!(templates(root.path()).resolve("on").is_err());
    }
}
