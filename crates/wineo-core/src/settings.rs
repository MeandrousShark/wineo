use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::bail;
use toml_edit::DocumentMut;

use crate::art::TargetCleanup;
use crate::config::{self, EnvironmentConfig, GlobalConfig, WINE_ONLY_KEYS, native_key_message};
use crate::desktop;
use crate::edit::{self, Scope};
use crate::environment::Environment;
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;
use crate::install::{self, Change, Wanted};
use crate::launch;
use crate::name;
use crate::operation::Locks;
use crate::paths::Paths;
use crate::prefix;
use crate::runner::Runner;
use crate::runners::Runners;

/// Which config file `update` edits.
#[derive(Debug, Clone, Copy)]
pub enum ConfigFile<'a> {
    Global,
    Environment(&'a str),
}

/// Validation failure that the caller may render with its own recovery hint.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("runner layouts are not compatible")]
    RunnerKindMismatch {
        current: String,
        changed: String,
        kind: String,
    },
}

/// Validates environment settings and reconciles installed components before
/// the config is written. Callers hold the environment lock while checking
/// and updating, so the prefix and config cannot diverge during a launch.
pub fn check(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<Vec<Change>> {
    check_native(paths, global, name, pairs, unset)?;
    let runner = check_runner(paths, global, name, pairs)?;
    check_desktop(paths, name, pairs, unset)?;
    apply_components(paths, global, name, runner.as_ref(), pairs, unset)
}

/// Settings that decide what the next launch runs, and against what. They all
/// have to exclude whatever is using the environment, so nothing launches
/// halfway between the old setting and the new one, and `dxvk` and `vkd3d`
/// additionally write into the prefix while they are applied.
pub fn changes_the_launch(pairs: &[(&str, &str)], unset: &[&str]) -> bool {
    ["prefix", "runner", "dxvk", "vkd3d"]
        .iter()
        .any(|key| pairs.iter().any(|(set, _)| set == key) || unset.contains(key))
}

/// The value a key will hold once this set is written, so a check can look at
/// the result rather than at the change.
fn value_after(
    current: Option<&str>,
    pairs: &[(&str, &str)],
    unset: &[&str],
    key: &str,
) -> Option<String> {
    if unset.contains(&key) {
        return None;
    }
    pairs
        .iter()
        .rev()
        .find(|(set, _)| *set == key)
        .map(|(_, raw)| unquote(raw).to_owned())
        .or_else(|| current.map(str::to_owned))
}

/// Applies `dxvk` and `vkd3d` before the config is written, so a failed
/// install leaves the config saying what the prefix actually has.
fn apply_components(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    runner: Option<&Runner>,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<Vec<Change>> {
    let environment = Environment::load(paths, name).ok();
    let config = environment.as_ref().map(|env| &env.config);
    let wanted = Wanted {
        dxvk: value_after(
            config.and_then(|config| config.wine()?.dxvk.as_deref()),
            pairs,
            unset,
            "dxvk",
        ),
        vkd3d: value_after(
            config.and_then(|config| config.wine()?.vkd3d.as_deref()),
            pairs,
            unset,
            "vkd3d",
        ),
    };
    // Nothing resolvable to check against, so leave it to the next launch.
    let Some(runner) = runner else {
        return Ok(Vec::new());
    };
    // Refused whether or not there is a prefix to install into yet.
    install::check(name, runner, &wanted)?;
    let Some(prefix) = environment.as_ref().and_then(|env| env.prefix(paths).ok()) else {
        return Ok(Vec::new());
    };
    // A prefix that does not exist yet is left to the next launch, which
    // reconciles anyway.
    if !prefix::is_prefix(&runner.wine_prefix(&prefix)) {
        return Ok(Vec::new());
    }
    Ok(install::reconcile(
        paths, global, name, runner, &prefix, &wanted,
    )?)
}

/// The targets a set turns a desktop entry on for.
fn enabled_desktops(pairs: &[(&str, &str)]) -> Vec<String> {
    pairs
        .iter()
        .filter_map(|(key, raw)| {
            let (target, field) = key.strip_prefix("targets.")?.split_once('.')?;
            (field == "desktop" && unquote(raw) == "true").then(|| target.to_owned())
        })
        .collect()
}

/// Refuses a `desktop = true` whose target points at something that is not
/// there, before anything is written, so a typo never leaves a config asking
/// for an entry that could not be made. A target with no `exe` at all is left
/// to the schema, which requires one.
fn check_desktop(
    paths: &Paths,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<()> {
    let enabled = enabled_desktops(pairs);
    if enabled.is_empty() {
        return Ok(());
    }
    desktop::supported(paths)?;
    let environment = Environment::load(paths, name).ok();
    let Some(prefix) = environment.as_ref().and_then(|env| env.prefix(paths).ok()) else {
        return Ok(());
    };
    // No prefix yet means nothing to resolve against, so the next launch,
    // which reconciles anyway, is where this gets checked.
    if !prefix::holds_prefix(&prefix) {
        return Ok(());
    }
    for target in enabled {
        let current = environment
            .as_ref()
            .and_then(|env| env.config.targets.get(&target))
            .map(|target| target.exe.as_str());
        if let Some(exe) = value_after(current, pairs, unset, &format!("targets.{target}.exe")) {
            desktop::check(paths, &prefix, &exe)?;
        }
    }
    Ok(())
}

/// Refuses the keys a native environment can't have, with the same line its
/// config would be refused with, and checks the exes it is given the way a
/// launch resolves them. Nothing to do for a Wine environment.
fn check_native(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
    unset: &[&str],
) -> anyhow::Result<()> {
    let Ok(environment) = Environment::load(paths, name) else {
        return Ok(());
    };
    if environment.config.wine().is_some() {
        return Ok(());
    }
    if let Some((key, _)) = pairs.iter().find(|(key, _)| WINE_ONLY_KEYS.contains(key)) {
        bail!("{}", native_key_message(name, key));
    }
    let mut targets: Vec<String> = pairs
        .iter()
        .filter_map(|(key, _)| {
            let (target, field) = key.strip_prefix("targets.")?.split_once('.')?;
            (field == "exe").then(|| target.to_owned())
        })
        .collect();
    targets.extend(enabled_desktops(pairs));
    for target in targets {
        let current = environment
            .config
            .targets
            .get(&target)
            .map(|target| target.exe.as_str());
        if let Some(exe) = value_after(current, pairs, unset, &format!("targets.{target}.exe")) {
            launch::check_native_exe(paths, global, name, &environment.config.env, &exe)?;
        }
    }
    Ok(())
}

pub fn changed_runner<'a>(pairs: &[(&'a str, &'a str)]) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| *key == "runner")
        .map(|(_, raw)| unquote(raw))
}

/// Quoting is optional on the command line, so both spellings mean the same.
fn unquote(raw: &str) -> &str {
    raw.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            raw.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(raw)
}

/// Settings only a Proton environment has anything to do with.
const PROTON_ONLY: [&str; 2] = ["gameid", "store"];

/// Resolves a new runner and refuses the settings that would not survive it:
/// a change across kinds, or protonfixes on something that is not Proton.
fn check_runner(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    pairs: &[(&str, &str)],
) -> anyhow::Result<Option<Runner>> {
    let changed = changed_runner(pairs);
    let runners = Runners::new(paths, global)?;
    let current = Environment::load(paths, name)
        .ok()
        .and_then(|environment| runners.resolve(&environment.wine().ok()?.runner).ok());

    let runner = match changed {
        Some(changed) => {
            let new = runners.resolve(changed)?;
            // A wine prefix has no `pfx/`, and a Proton one keeps nothing where
            // a plain wine looks for it, so crossing kinds is worse than not
            // migrating.
            if let Some(current) = &current
                && current.is_proton() != new.is_proton()
            {
                bail!(CheckError::RunnerKindMismatch {
                    current: current.name().to_owned(),
                    changed: changed.to_owned(),
                    kind: new.kind_name().to_owned(),
                });
            }
            new
        }
        // Nothing resolvable to check against, and the write is still valid
        // TOML, so this is not the place to complain.
        None => match current {
            Some(current) => current,
            None => return Ok(None),
        },
    };
    let proton_only = pairs.iter().any(|(key, _)| PROTON_ONLY.contains(key));

    if proton_only && !runner.is_proton() {
        let key = pairs
            .iter()
            .find_map(|(key, _)| PROTON_ONLY.contains(key).then_some(*key))
            .expect("checked above");
        bail!(
            "`{key}` only means something to Proton, and `{name}` runs on {}",
            runner.name()
        );
    }
    Ok(Some(runner))
}

/// Where a `prefix=` setting would point.
pub fn new_prefix(paths: &Paths, raw: &str) -> Option<PathBuf> {
    paths.absolute(unquote(raw)).ok()
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
    let targets_before = target_names(&doc);

    for (key, value) in set {
        let value = match (scope, *key) {
            // Stored the same way `wineo new` writes them.
            (Scope::Environment, "prefix") => paths.store_prefix(&paths.absolute(value)?)?,
            (Scope::Global, "defaults.prefix_root") => {
                let dir = paths.absolute(value)?;
                paths.check_prefix_root(&dir)?;
                paths.store(&dir)?
            }
            (Scope::Environment, key)
                if key.starts_with("targets.")
                    && (key.ends_with(".exe") || key.ends_with(".cwd")) =>
            {
                paths.store_value(value)?
            }
            _ => (*value).to_owned(),
        };
        edit::set(&mut doc, scope, key, &value)?;
    }
    if paths.portable_root().is_some() && scope == Scope::Environment {
        for (key, _) in set {
            let Some((target, "desktop")) = key
                .strip_prefix("targets.")
                .and_then(|rest| rest.rsplit_once('.'))
            else {
                continue;
            };
            let enabled = doc
                .get("targets")
                .and_then(|targets| targets.get(target))
                .and_then(|table| table.get("desktop"))
                .and_then(|value| value.as_bool());
            if enabled == Some(true) {
                return Err(Error::PortableDesktop);
            }
        }
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
    let cleanup = match file {
        ConfigFile::Environment(env) => {
            let targets_after = target_names(&doc);
            let removed: Vec<String> = targets_before.difference(&targets_after).cloned().collect();
            if removed.is_empty() {
                None
            } else {
                Some(TargetCleanup::prepare(paths, env, removed)?)
            }
        }
        ConfigFile::Global => None,
    };
    if changed {
        write_atomic(&path, &updated)?;
    }
    if let Some(cleanup) = cleanup {
        cleanup.finish()?;
    }
    Ok(missing)
}

fn target_names(doc: &DocumentMut) -> BTreeSet<String> {
    doc.get("targets")
        .and_then(|targets| targets.as_table_like())
        .map(|targets| targets.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default()
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
    fn a_no_op_edit_leaves_a_versioned_wine_file_byte_identical() {
        let (_root, paths) = setup();
        let text = format!(
            "{NOTEPAD}schema_version = 1\ngamemode = true # mine\n\n[targets.notepad]\nexe = \"notepad\"\n"
        );
        write_env(&paths, &text);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("gamemode", "true"), ("runner", "wine")],
            &[],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            text
        );
    }

    #[test]
    fn an_edit_never_adds_kind_to_a_wine_file() {
        let (_root, paths) = setup();
        write_env(&paths, NOTEPAD);
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("gamemode", "true")],
            &[],
        )
        .unwrap();
        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert!(!text.contains("kind"), "{text}");
    }

    #[test]
    fn a_wine_only_key_is_not_written_to_a_native_file() {
        let (_root, paths) = setup();
        let text = "kind = \"native\"\n";
        write_env(&paths, text);
        let err = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("runner", "wine")],
            &[],
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .ends_with("native environment notepad can't set runner"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            text
        );
    }

    #[test]
    fn native_wayland_is_refused_at_both_levels_without_writing() {
        let (_root, paths) = setup();
        let text = "kind = 'native'\n[targets.game]\nexe = 'game'\n";
        write_env(&paths, text);
        for key in ["wayland", "targets.game.wayland"] {
            let err = update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[(key, "true")],
                &[],
            )
            .unwrap_err();
            assert!(
                err.to_string().contains(&format!("can't set {key}")),
                "{err}"
            );
            assert_eq!(
                fs::read_to_string(paths.environment_config("notepad")).unwrap(),
                text
            );
        }
    }

    #[test]
    fn gamescope_false_becomes_a_table_and_an_unset_field_keeps_it_on() {
        let (_root, paths) = setup();
        write_env(
            &paths,
            &format!(
                "{NOTEPAD}gamescope = false # keep\n[targets.game]\nexe = 'game'\ngamescope = false # keep target\n"
            ),
        );
        for prefix in ["gamescope", "targets.game.gamescope"] {
            let field = format!("{prefix}.width");
            update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[(field.as_str(), "1920")],
                &[],
            )
            .unwrap();
            update(
                &paths,
                ConfigFile::Environment("notepad"),
                &[],
                &[field.as_str()],
            )
            .unwrap();
        }
        let text = fs::read_to_string(paths.environment_config("notepad")).unwrap();
        assert!(text.contains("[gamescope]"), "{text}");
        assert!(text.contains("[targets.game.gamescope]"), "{text}");
        assert!(text.contains("# keep"), "{text}");
        assert!(text.contains("# keep target"), "{text}");
        let config = EnvironmentConfig::parse(Path::new("/x"), &text).unwrap();
        assert!(config.gamescope.unwrap().on().is_some());
        assert!(
            config.targets["game"]
                .gamescope
                .as_ref()
                .unwrap()
                .on()
                .is_some()
        );
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

    #[test]
    fn unsetting_a_target_removes_its_art() {
        let (_root, paths) = setup();
        write_env(
            &paths,
            &format!("{NOTEPAD}[targets.a]\nexe = \"a\"\n[targets.b]\nexe = \"b\"\n"),
        );
        let art = paths.art_dir("notepad");
        for target in ["a", "b"] {
            fs::create_dir_all(art.join(target)).unwrap();
            fs::write(art.join(target).join("icon.png"), b"x").unwrap();
        }
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("targets.b.exe", "b2")],
            &["targets.a.cwd"],
        )
        .unwrap();
        assert!(art.join("a").exists() && art.join("b").exists());
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[],
            &["targets.a"],
        )
        .unwrap();
        assert!(!art.join("a").exists());
        assert!(art.join("b/icon.png").exists());
    }

    #[test]
    fn an_invalid_art_toml_stops_a_target_removal_first() {
        let (_root, paths) = setup();
        let text = format!("{NOTEPAD}[targets.a]\nexe = \"a\"\n");
        write_env(&paths, &text);
        let art = paths.art_dir("notepad");
        fs::create_dir_all(art.join("a")).unwrap();
        fs::write(art.join("art.toml"), "schema_version = 1\ncolor = 1\n").unwrap();
        let err = update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[],
            &["targets.a"],
        )
        .unwrap_err();
        assert!(matches!(err, Error::ConfigSyntax { .. }), "{err}");
        assert_eq!(
            fs::read_to_string(paths.environment_config("notepad")).unwrap(),
            text
        );
        assert!(art.join("a").exists());

        // Nothing to clean up means art.toml is not read at all.
        update(
            &paths,
            ConfigFile::Environment("notepad"),
            &[("targets.a.cwd", "C:\\")],
            &[],
        )
        .unwrap();
    }
}
