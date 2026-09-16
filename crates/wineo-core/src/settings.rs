use std::fs;
use std::io;
use std::path::PathBuf;

use toml_edit::DocumentMut;

use crate::config::{EnvironmentConfig, GlobalConfig};
use crate::edit::{self, Scope};
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::name;
use crate::paths::Paths;

/// Which config file `update` edits.
#[derive(Debug, Clone, Copy)]
pub enum ConfigFile<'a> {
    Global,
    Environment(&'a str),
}

/// Applies `key=value` settings and removals to a config file, keeping its
/// formatting. Nothing is written unless the result passes validation.
/// Returns the keys asked to be removed that weren't set.
pub fn update(
    paths: &Paths,
    file: ConfigFile,
    set: &[(&str, &str)],
    unset: &[&str],
) -> Result<Vec<String>> {
    let (scope, path) = locate(paths, file)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound && scope == Scope::Global => {
            String::new()
        }
        Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().map_err(|source| Error::ConfigParse {
        path: path.clone(),
        source: Box::new(source),
    })?;

    for (key, value) in set {
        let value = match (scope, *key) {
            // Stored the same way `wineo new` writes them.
            (Scope::Environment, "prefix") | (Scope::Global, "defaults.prefix_root") => {
                paths.contract(&paths.absolute(value)?)?
            }
            _ => (*value).to_owned(),
        };
        edit::set(&mut doc, scope, key, &value)?;
    }
    let mut missing = Vec::new();
    for key in unset {
        if !edit::unset(&mut doc, scope, key)? {
            missing.push((*key).to_owned());
        }
    }

    let updated = doc.to_string();
    match scope {
        Scope::Global => {
            GlobalConfig::parse(&path, &updated)?;
        }
        Scope::Environment => {
            EnvironmentConfig::parse(&path, &updated)?;
        }
    }
    if updated != text {
        write_atomic(&path, &updated)?;
    }
    Ok(missing)
}

fn locate(paths: &Paths, file: ConfigFile) -> Result<(Scope, PathBuf)> {
    match file {
        ConfigFile::Global => Ok((Scope::Global, paths.global_config())),
        ConfigFile::Environment(env) => {
            if !name::is_valid(env) {
                return Err(Error::InvalidName(env.to_owned()));
            }
            let path = paths.environment_config(env);
            if !path.exists() {
                return Err(Error::UnknownEnvironment(env.to_owned()));
            }
            Ok((Scope::Environment, path))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::*;

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        (root, paths)
    }

    const NOTEPAD: &str =
        "# hand written\nprefix = \"~/prefixes/notepad\"\nrunner = \"wine\"\narch = \"win64\"\n";

    fn write_env(paths: &Paths, text: &str) {
        let path = paths.environment_config("notepad");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn sets_values_and_keeps_comments() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        let missing = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[
                ("targets.notepad.exe", "notepad"),
                ("env.WINEDEBUG", "-all"),
            ],
            &["env.NOPE"],
        )
        .unwrap();
        assert_eq!(missing, ["env.NOPE"]);

        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert!(text.starts_with("# hand written\n"), "{text}");
        let config = EnvironmentConfig::parse(Path::new("/x"), &text).unwrap();
        assert_eq!(config.targets["notepad"].exe, "notepad");
        assert_eq!(config.env["WINEDEBUG"], "-all");
    }

    #[test]
    fn invalid_results_are_not_written() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        let err = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("targets.a.default", "true")],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, Error::ConfigSyntax { .. }), "{err}");
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            NOTEPAD
        );
    }

    #[test]
    fn prefix_paths_are_stored_relative_to_home() {
        let (root, paths) = setup();
        write_env(&paths, NOTEPAD);
        let elsewhere = root.path().join("Games/notepad");
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("prefix", elsewhere.to_str().unwrap())],
            &[],
        )
        .unwrap();
        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert!(text.contains("prefix = \"~/Games/notepad\""), "{text}");
    }

    #[test]
    fn global_config_is_created_on_first_set() {
        let (_root, paths) = setup();
        update(&paths, ConfigFile::Global, &[("stop_timeout", "30")], &[]).unwrap();
        let config = GlobalConfig::load(&paths).unwrap();
        assert_eq!(config.stop_timeout, 30);
    }

    #[test]
    fn unknown_environments_are_errors() {
        let (_root, paths) = setup();
        assert!(matches!(
            update(
                &paths,
                ConfigFile::Environment("nope"),
                &[("runner", "wine")],
                &[]
            ),
            Err(Error::UnknownEnvironment(_))
        ));
    }
}
