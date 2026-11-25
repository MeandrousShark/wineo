use std::ffi::OsString;
use std::path::Path;

use super::*;

#[test]
fn versions_compare_by_their_numbers() {
    assert_eq!(Version::parse("0.21.1"), Some(Version::new(0, 21, 1)));
    assert_eq!(Version::parse("3.10"), Some(Version::new(3, 10, 0)));
    assert_eq!(Version::parse("3.14.10+"), Some(Version::new(3, 14, 10)));
    assert_eq!(Version::parse("0.22.0rc1"), Some(Version::new(0, 22, 0)));
    assert_eq!(Version::parse("1"), None);
    assert_eq!(Version::parse("x.1"), None);
    assert_eq!(Version::parse(""), None);
    assert!(Version::new(0, 20, 34) < FLOOR);
    assert!(Version::new(0, 21, 10) > FLOOR);
    assert!(Version::new(1, 0, 0) > FLOOR);
    assert_eq!(FLOOR.to_string(), "0.21.1");
}

fn paths(home: &Path) -> Paths {
    let home = home.to_owned();
    Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        None,
    )
    .unwrap()
}

#[test]
fn legendary_gets_its_own_variables_and_keeps_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let env = Env::from_pairs(&[
        ("PATH", "/usr/bin"),
        ("LGDRY_WRAPPER", "x"),
        ("LEGENDARY_WRAPPER_EXE", "x"),
        ("LEGENDARY_CONFIG_PATH", "/elsewhere"),
        ("PYTHONPATH", "/x"),
        ("PYTHONHOME", "/x"),
        ("XDG_CACHE_HOME", "/cache"),
        ("LANG", "de_DE.UTF-8"),
        ("LC_ALL", "de_DE.UTF-8"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("REQUESTS_CA_BUNDLE", "/ca.pem"),
        ("MY_PYTHON", "kept"),
    ]);
    let vars: Vec<(String, String)> = env
        .for_legendary(&paths)
        .into_iter()
        .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
        .collect();
    let get = |key: &str| -> Vec<&str> {
        vars.iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .collect()
    };
    for gone in [
        "LGDRY_WRAPPER",
        "LEGENDARY_WRAPPER_EXE",
        "PYTHONPATH",
        "PYTHONHOME",
    ] {
        assert!(get(gone).is_empty(), "{gone} reached Legendary");
    }
    let state = paths.host_data().join("epic/legendary");
    assert_eq!(get("LEGENDARY_CONFIG_PATH"), [state.to_str().unwrap()]);
    let cache = paths.host_data().join("epic/cache");
    assert_eq!(get("XDG_CACHE_HOME"), [cache.to_str().unwrap()]);
    for (kept, value) in [
        ("PATH", "/usr/bin"),
        ("LANG", "de_DE.UTF-8"),
        ("LC_ALL", "de_DE.UTF-8"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("REQUESTS_CA_BUNDLE", "/ca.pem"),
        ("MY_PYTHON", "kept"),
    ] {
        assert_eq!(get(kept), [value], "{kept}");
    }
}

#[test]
fn the_epic_folder_is_the_hosts_in_a_portable_root() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = root.path().join("portable");
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&portable),
    )
    .unwrap();
    let data = root.path().join(".local/share/wineo");
    assert_eq!(paths.epic_dir(), data.join("epic"));
    assert_eq!(state_dir(&paths), data.join("epic/legendary"));
    assert_eq!(
        own_copy::file(&paths, "0.21.1"),
        data.join("epic/bin/legendary-0.21.1")
    );
    assert!(!paths.epic_owned_cache().starts_with(&portable));
}

#[test]
fn env_debug_names_variables_without_their_values() {
    let env = Env::from_pairs(&[("TOKEN", "hunter2")]);
    let shown = format!("{env:?}");
    assert!(shown.contains("TOKEN"), "{shown}");
    assert!(!shown.contains("hunter2"), "{shown}");
}
