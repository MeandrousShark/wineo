use std::fs;
use std::io;
use std::path::PathBuf;

use crate::config::{self, EnvironmentConfig, GlobalConfig};
use crate::edit::{self, Scope};
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::name;
use crate::operation::Locks;
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
    let (scope, path) = config_path(paths, file)?;
    // Read under the lock, so a change or a removal that lands in the meantime
    // is built on or refused rather than overwritten.
    let _guard = Locks::new(paths).config(&path)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => match file {
            ConfigFile::Global => None,
            ConfigFile::Environment(env) => {
                return Err(Error::UnknownEnvironment(env.to_owned()));
            }
        },
        Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
    };
    let mut doc = match &text {
        Some(text) => {
            config::check_schema(&path, text)?;
            text.parse().map_err(|source| Error::ConfigParse {
                path: path.clone(),
                source: Box::new(source),
            })?
        }
        None => config::new_document(),
    };
    let before = doc.to_string();

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

    let mut updated = doc.to_string();
    let changed = updated != before;
    if changed {
        config::stamp_schema(&mut doc);
        updated = doc.to_string();
    }
    match scope {
        Scope::Global => {
            GlobalConfig::parse(&path, &updated)?;
        }
        Scope::Environment => {
            EnvironmentConfig::parse(&path, &updated)?;
        }
    }
    if changed {
        write_atomic(&path, &updated)?;
    }
    Ok(missing)
}

fn config_path(paths: &Paths, file: ConfigFile) -> Result<(Scope, PathBuf)> {
    match file {
        ConfigFile::Global => Ok((Scope::Global, paths.global_config())),
        ConfigFile::Environment(env) => {
            if !name::is_valid(env) {
                return Err(Error::InvalidName(env.to_owned()));
            }
            Ok((Scope::Environment, paths.environment_config(env)))
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

    // Without the lock, a removal between the read and the write would have the
    // write bring the config back.
    #[test]
    fn a_change_waits_for_a_removal_and_does_not_bring_the_config_back() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        let config = paths.environment_config("notepad");
        let held = Locks::new(&paths).config(&config).unwrap();
        let change = {
            let paths = paths.clone();
            std::thread::spawn(move || {
                update(
                    &paths,
                    ConfigFile::Environment("notepad"),
                    &[("runner", "wine")],
                    &[],
                )
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::remove_file(&config).unwrap();
        drop(held);
        assert!(matches!(
            change.join().unwrap(),
            Err(Error::UnknownEnvironment(_))
        ));
        assert!(!config.exists());
    }

    fn modified(path: &Path) -> std::time::SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn an_edit_stamps_a_legacy_file_and_keeps_its_formatting() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("runner", "wine-tkg")],
            &[],
        )
        .unwrap();
        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert_eq!(
            text,
            "# hand written\nprefix = \"~/prefixes/notepad\"\nrunner = \"wine-tkg\"\narch = \"win64\"\nschema_version = 1\n"
        );
        assert_eq!(
            EnvironmentConfig::parse(Path::new("/x"), &text)
                .unwrap()
                .schema_version,
            1
        );
    }

    #[test]
    fn a_no_op_edit_leaves_a_legacy_file_alone() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        let path = paths.environment_config("notepad");
        let before = modified(&path);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let missing = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("runner", "wine")],
            &["env.NOPE"],
        )
        .unwrap();
        assert_eq!(missing, ["env.NOPE"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), NOTEPAD);
        assert_eq!(modified(&path), before);
    }

    #[test]
    fn a_versioned_file_is_not_stamped_twice() {
        let (_root, paths) = setup();
        let versioned = format!("{NOTEPAD}schema_version = 1\n");
        write_env(&paths, &versioned);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("gamemode", "true")],
            &[],
        )
        .unwrap();
        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert_eq!(text, format!("{versioned}gamemode = true\n"));
    }

    #[test]
    fn a_new_global_config_starts_with_the_version() {
        let (_root, paths) = setup();
        update(&paths, ConfigFile::Global, &[("stop_timeout", "30")], &[]).unwrap();
        assert_eq!(
            fs::read_to_string(paths.global_config()).unwrap(),
            "schema_version = 1\nstop_timeout = 30\n"
        );
    }

    #[test]
    fn a_no_op_edit_creates_no_global_config() {
        let (_root, paths) = setup();
        let missing = update(&paths, ConfigFile::Global, &[], &["gamemode"]).unwrap();
        assert_eq!(missing, ["gamemode"]);
        assert!(!paths.global_config().exists());
    }

    #[test]
    fn a_legacy_global_config_is_stamped_on_edit() {
        let (_root, paths) = setup();
        let original = "# mine\n[defaults]\nrunner = \"wine\"\n";
        fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
        fs::write(paths.global_config(), original).unwrap();
        update(
            &paths,
            ConfigFile::Global,
            &[("defaults.arch", "win32")],
            &[],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(paths.global_config()).unwrap(),
            "schema_version = 1\n# mine\n[defaults]\nrunner = \"wine\"\narch = \"win32\"\n"
        );
    }

    #[test]
    fn the_version_is_not_a_setting() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        for (set, unset) in [
            (vec![("schema_version", "2")], vec![]),
            (vec![], vec!["schema_version"]),
        ] {
            assert!(matches!(
                update(&paths, ConfigFile::Environment("notepad"), &set, &unset),
                Err(Error::Edit(edit::EditError::UnknownKey(_)))
            ));
        }
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            NOTEPAD
        );
    }

    #[test]
    fn unsupported_versions_are_never_edited() {
        let (_root, paths) = setup();
        for version in ["0", "2", "\"1\""] {
            let text = format!("schema_version = {version}\n{NOTEPAD}");
            write_env(&paths, &text);
            let path = paths.environment_config("notepad");
            let before = modified(&path);
            let err = update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[("runner", "wine-tkg")],
                &[],
            )
            .unwrap_err();
            assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
            assert_eq!(modified(&path), before);

            fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
            fs::write(
                paths.global_config(),
                format!("schema_version = {version}\n"),
            )
            .unwrap();
            let err =
                update(&paths, ConfigFile::Global, &[("stop_timeout", "3")], &[]).unwrap_err();
            assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
            assert_eq!(
                fs::read_to_string(paths.global_config()).unwrap(),
                format!("schema_version = {version}\n")
            );
        }
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
