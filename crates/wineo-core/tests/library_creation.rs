use std::ffi::OsString;
use std::fs;

use tempfile::tempdir;
use wineo_core::clone;
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::library;
use wineo_core::paths::Paths;
use wineo_core::trash;

fn paths(root: &std::path::Path) -> Paths {
    let home = root.to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn create(paths: &Paths) -> Vec<String> {
    let mut warnings = Vec::new();
    Environment::create_native(
        paths,
        &GlobalConfig::default(),
        "hades",
        None,
        false,
        |warning| warnings.push(warning),
    )
    .unwrap();
    warnings
}

fn starred_library(paths: &Paths) -> Vec<u8> {
    create(paths);
    library::set_favorite(paths, "hades", true).unwrap();
    library::set_hidden(paths, "hades", true).unwrap();
    fs::read(paths.library_file()).unwrap()
}

#[test]
fn failed_cleanup_then_readable_name_reuse_replaces_stale_entry() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let mut broken = starred_library(&paths);
    broken.extend_from_slice(b"\n[broken\n");
    fs::write(paths.library_file(), &broken).unwrap();
    let expected_error = wineo_core::error::chain(&library::fresh(&paths, "hades").unwrap_err());

    let removed = trash::remove(&paths, "hades", false).unwrap();
    assert!(
        removed
            .warnings
            .iter()
            .any(|warning| warning.contains(&expected_error))
    );
    assert_eq!(fs::read(paths.library_file()).unwrap(), broken);

    fs::write(
        paths.library_file(),
        &broken[..broken.len() - b"\n[broken\n".len()],
    )
    .unwrap();
    assert!(create(&paths).is_empty());
    let entry = &library::load(&paths).environments["hades"];
    assert!(!entry.favorite);
    assert!(!entry.hidden);
    assert!(entry.added.is_some());
}

#[test]
fn reuse_while_library_is_broken_retains_old_flags_after_repair() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let valid = starred_library(&paths);
    let old_added = library::load(&paths).environments["hades"].added;
    let mut broken = valid.clone();
    broken.extend_from_slice(b"\n[broken\n");
    fs::write(paths.library_file(), &broken).unwrap();
    let expected_error = wineo_core::error::chain(&library::fresh(&paths, "hades").unwrap_err());

    let removed = trash::remove(&paths, "hades", false).unwrap();
    assert!(
        removed
            .warnings
            .iter()
            .any(|warning| warning.contains(&expected_error))
    );
    let warnings = create(&paths);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0],
        format!(
            "Can't update library.toml: {expected_error}. hades may show an old star or hidden flag once it's repaired."
        )
    );
    assert_eq!(fs::read(paths.library_file()).unwrap(), broken);

    fs::write(paths.library_file(), valid).unwrap();
    let entry = &library::load(&paths).environments["hades"];
    assert!(entry.favorite);
    assert!(entry.hidden);
    assert_eq!(entry.added, old_added);

    let removed = trash::remove(&paths, "hades", false).unwrap();
    let manifest =
        fs::read_to_string(paths.trash_dir().join(removed.id).join("manifest.toml")).unwrap();
    assert!(manifest.contains("favorite = true"));
    assert!(manifest.contains("hidden = true"));
}

#[test]
fn native_clone_starts_fresh_and_warns_if_library_is_broken() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    create(&paths);
    library::set_favorite(&paths, "hades", true).unwrap();
    library::set_hidden(&paths, "hades", true).unwrap();
    let review = clone::inspect_native(&paths, "hades", "copy").unwrap();
    let mut warnings = Vec::new();
    clone::execute_native(&paths, &review, |warning| warnings.push(warning)).unwrap();
    assert!(warnings.is_empty());
    let entry = &library::load(&paths).environments["copy"];
    assert!(!entry.favorite);
    assert!(!entry.hidden);
    assert!(entry.added.is_some());

    let mut broken = fs::read(paths.library_file()).unwrap();
    broken.extend_from_slice(b"\n[broken\n");
    fs::write(paths.library_file(), &broken).unwrap();
    let expected_error = wineo_core::error::chain(&library::fresh(&paths, "another").unwrap_err());
    let review = clone::inspect_native(&paths, "hades", "another").unwrap();
    clone::execute_native(&paths, &review, |warning| warnings.push(warning)).unwrap();
    assert!(paths.environment_config("another").exists());
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains(&expected_error));
    assert_eq!(fs::read(paths.library_file()).unwrap(), broken);
}
