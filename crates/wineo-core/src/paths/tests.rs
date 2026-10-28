use std::collections::HashMap;

use super::*;

#[test]
fn contract_text_contracts_whole_paths_under_home() {
    let paths = paths(&[("HOME", "/home/ann")]).unwrap();
    assert_eq!(
        paths.contract_text("invalid config /home/ann/.config/wineo/x.toml: bad"),
        "invalid config ~/.config/wineo/x.toml: bad"
    );
    assert_eq!(paths.contract_text("/home/ann"), "~");
    assert_eq!(
        paths.contract_text("`/home/ann/a` and /home/ann/b\n/home/ann."),
        "`~/a` and ~/b\n~."
    );
    assert_eq!(
        paths.contract_text("/home/ann2/x /mnt/home/ann/y /home/ann.old"),
        "/home/ann2/x /mnt/home/ann/y /home/ann.old"
    );
}

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
        paths.library_file(),
        Path::new("/home/me/.config/wineo/library.toml")
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
        paths.art_dir("notepad"),
        Path::new("/home/me/.local/share/wineo/art/notepad")
    );
    assert_eq!(
        paths.art_cache_dir(),
        Path::new("/home/me/.cache/wineo/art")
    );
    assert_eq!(
        paths.stats_log("notepad"),
        Path::new("/home/me/.local/share/wineo/stats/notepad.jsonl")
    );
    assert_eq!(
        paths.runtime_dir("notepad"),
        Path::new("/home/me/.local/state/wineo/notepad")
    );
    assert_eq!(
        paths.import_log("palworld"),
        Path::new("/home/me/.local/state/wineo/.imports/palworld.log")
    );
    // Removing an environment named imports must not take every log with it.
    assert!(
        !paths
            .import_log("palworld")
            .starts_with(paths.runtime_dir("imports"))
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
    assert_eq!(paths.art_dir("x"), Path::new("/data/wineo/art/x"));
    assert_eq!(paths.stats_log("x"), Path::new("/data/wineo/stats/x.jsonl"));
    assert_eq!(paths.host_data(), Path::new("/data/wineo"));
}

#[test]
fn art_cache_follows_xdg_cache_home_and_ignores_relative_values() {
    let paths = paths(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")]).unwrap();
    assert_eq!(paths.art_cache_dir(), Path::new("/cache/wineo/art"));
    let paths = self::paths(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "relative")]).unwrap();
    assert_eq!(
        paths.art_cache_dir(),
        Path::new("/home/me/.cache/wineo/art")
    );
}

#[test]
fn gog_covers_follow_xdg_cache_home_beside_the_art_cache() {
    let paths = paths(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")]).unwrap();
    assert_eq!(paths.gog_covers_dir(), Path::new("/cache/wineo/gog-covers"));
    let paths = self::paths(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "relative")]).unwrap();
    assert_eq!(
        paths.gog_covers_dir(),
        Path::new("/home/me/.cache/wineo/gog-covers")
    );
    assert!(!paths.gog_covers_dir().starts_with(paths.art_cache_dir()));
}

#[test]
fn gog_covers_stay_on_the_host_in_a_portable_folder() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    assert_eq!(
        paths.gog_covers_dir(),
        Path::new("/home/me/.cache/wineo/gog-covers")
    );
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

fn portable(vars: &[(&str, &str)], option: Option<&str>) -> Result<Paths> {
    let vars: HashMap<String, OsString> = vars
        .iter()
        .map(|(key, value)| (key.to_string(), OsString::from(value)))
        .collect();
    Paths::select(|key| vars.get(key).cloned(), option.map(Path::new))
}

#[test]
fn option_wins_over_the_variable() {
    let home = [("HOME", "/home/me"), (PORTABLE_VAR, "/env/folder")];
    let paths = portable(&home, Some("/opt/folder")).unwrap();
    assert_eq!(paths.portable_root(), Some(Path::new("/opt/folder")));
    let paths = portable(&home, None).unwrap();
    assert_eq!(paths.portable_root(), Some(Path::new("/env/folder")));
}

#[test]
fn no_selector_keeps_xdg_and_an_empty_one_is_handled() {
    let paths = portable(&[("HOME", "/home/me")], None).unwrap();
    assert_eq!(paths.portable_root(), None);
    let paths = portable(&[("HOME", "/home/me"), (PORTABLE_VAR, "")], None).unwrap();
    assert_eq!(paths.portable_root(), None);
    assert_eq!(
        paths.global_config(),
        Path::new("/home/me/.config/wineo/config.toml")
    );
    assert!(matches!(
        portable(&[("HOME", "/home/me")], Some("")),
        Err(Error::EmptyPortable)
    ));
}

#[test]
fn selected_roots_become_absolute() {
    let cwd = std::env::current_dir().unwrap();
    let paths = portable(&[("HOME", "/home/me")], Some("rel/folder")).unwrap();
    assert_eq!(
        paths.portable_root(),
        Some(cwd.join("rel/folder").as_path())
    );
    let paths = portable(&[("HOME", "/home/me"), (PORTABLE_VAR, "~/folder")], None).unwrap();
    assert_eq!(paths.portable_root(), Some(Path::new("/home/me/folder")));
}

#[test]
fn portable_layout_keeps_host_data_and_runtime_on_the_host() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    assert_eq!(
        paths.global_config(),
        Path::new("/mnt/usb/wineo/config/config.toml")
    );
    assert_eq!(
        paths.environment_config("hades"),
        Path::new("/mnt/usb/wineo/config/environments/hades.toml")
    );
    assert_eq!(
        paths.library_file(),
        Path::new("/mnt/usb/wineo/config/library.toml")
    );
    assert_eq!(
        paths.templates_dir(),
        Path::new("/mnt/usb/wineo/config/templates")
    );
    assert_eq!(
        paths.themes_dir(),
        Path::new("/mnt/usb/wineo/config/themes")
    );
    assert_eq!(
        paths.default_prefix_root(),
        Path::new("/mnt/usb/wineo/prefixes")
    );
    assert_eq!(
        paths.snapshot_root(&paths.default_prefix_root().join("hades"), "hades"),
        Path::new("/mnt/usb/wineo/prefixes/.wineo-snapshots/hades")
    );
    assert_eq!(
        paths.default_runner_root(),
        Path::new("/home/me/.local/share/wineo/runners")
    );
    assert_eq!(
        paths.applications_dir(),
        Path::new("/home/me/.local/share/applications")
    );
    assert_eq!(paths.icons_dir(), Path::new("/home/me/.local/share/icons"));
    assert_eq!(
        paths.art_dir("hades"),
        Path::new("/mnt/usb/wineo/art/hades")
    );
    assert_eq!(
        paths.stats_log("hades"),
        Path::new("/mnt/usb/wineo/stats/hades.jsonl")
    );
    assert_eq!(paths.host_data(), Path::new("/home/me/.local/share/wineo"));
    assert_eq!(
        paths.art_cache_dir(),
        Path::new("/home/me/.cache/wineo/art")
    );
    assert_eq!(
        paths.gog_download_cache(),
        Path::new("/home/me/.cache/wineo/gog-downloads")
    );
    let state = Path::new("/home/me/.local/state/wineo/.portable");
    assert!(paths.runtime_dir("hades").starts_with(state));
    assert!(paths.locks_dir().starts_with(state));
    assert!(paths.import_log("hades").starts_with(state));
}

#[test]
fn runtime_state_is_isolated_per_root() {
    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    let one = portable(&[("HOME", "/home/me")], Some("/mnt/a/wineo")).unwrap();
    let two = portable(&[("HOME", "/home/me")], Some("/mnt/b/wineo")).unwrap();
    let again = portable(&[("HOME", "/home/me")], Some("/mnt/a/wineo")).unwrap();
    for (left, right) in [(&xdg, &one), (&xdg, &two), (&one, &two)] {
        assert_ne!(left.runtime_dir("hades"), right.runtime_dir("hades"));
        assert_ne!(left.locks_dir(), right.locks_dir());
        assert_ne!(left.import_log("hades"), right.import_log("hades"));
        assert!(
            !right
                .runtime_dir("hades")
                .starts_with(left.runtime_dir("hades"))
        );
    }
    assert_eq!(one.runtime_dir("hades"), again.runtime_dir("hades"));
    assert_eq!(one.locks_dir(), again.locks_dir());
}

#[test]
fn portable_paths_are_stored_relative_to_the_root() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    assert_eq!(
        paths
            .store(Path::new("/mnt/usb/wineo/prefixes/hades"))
            .unwrap(),
        "prefixes/hades"
    );
    assert_eq!(paths.store(Path::new("/mnt/usb/wineo")).unwrap(), ".");
    assert_eq!(
        paths.store(Path::new("/home/me/Games/x")).unwrap(),
        "~/Games/x"
    );
    assert_eq!(paths.store(Path::new("/opt/game")).unwrap(), "/opt/game");
    assert_eq!(
        paths.store(Path::new("/mnt/usb/wineo2/x")).unwrap(),
        "/mnt/usb/wineo2/x"
    );

    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    assert_eq!(
        xdg.store(Path::new("/home/me/.local/share/wineo/prefixes/x"))
            .unwrap(),
        "~/.local/share/wineo/prefixes/x"
    );
}

#[test]
fn stored_paths_resolve_from_the_root_not_the_current_dir() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert_ne!(cwd, Path::new("/mnt/usb/wineo"));
    let resolve = |value| paths.resolve_stored(value).unwrap();
    assert_eq!(
        resolve("prefixes/hades"),
        Some(PathBuf::from("/mnt/usb/wineo/prefixes/hades"))
    );
    assert_eq!(resolve("."), Some(PathBuf::from("/mnt/usb/wineo/.")));
    assert_eq!(resolve("/opt/game"), Some(PathBuf::from("/opt/game")));
    assert_eq!(
        resolve("~/Games/x"),
        Some(PathBuf::from("/home/me/Games/x"))
    );
    assert_eq!(resolve("C:\\Games\\x.exe"), None);
    assert_eq!(resolve("c:/Games/x.exe"), None);

    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    assert_eq!(
        xdg.resolve_stored("some/dir").unwrap(),
        Some(cwd.join("some/dir"))
    );
    assert_eq!(xdg.resolve_stored("C:\\x.exe").unwrap(), None);
}

#[test]
fn prefix_locks_are_host_wide() {
    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    let one = portable(&[("HOME", "/home/me")], Some("/mnt/a/wineo")).unwrap();
    assert_eq!(xdg.prefix_locks_dir(), xdg.locks_dir());
    assert_eq!(one.prefix_locks_dir(), xdg.prefix_locks_dir());
    assert_eq!(one.gog_credentials_lock(), xdg.gog_credentials_lock());
    assert_eq!(one.gog_folders_dir(), xdg.gog_folders_dir());
    assert_eq!(
        xdg.gog_folders_dir(),
        Path::new("/home/me/.local/share/wineo/gog/folders")
    );
    assert_eq!(one.gog_folder_lock("ab"), xdg.gog_folder_lock("ab"));
    assert_eq!(
        xdg.gog_folder_lock("ab"),
        Path::new("/home/me/.local/state/wineo/.locks/gog-folder-ab.lock")
    );
    assert_eq!(
        one.gog_folder_in_use_lock("ab"),
        xdg.gog_folder_in_use_lock("ab")
    );
    assert_eq!(
        xdg.gog_folder_in_use_lock("ab"),
        Path::new("/home/me/.local/state/wineo/.locks/gog-folder-ab.in-use.lock")
    );
    assert_eq!(one.gog_folder_holders("ab"), xdg.gog_folder_holders("ab"));
    assert_eq!(
        xdg.gog_folder_holders("ab"),
        Path::new("/home/me/.local/state/wineo/.locks/gog-folder-ab.holders")
    );
    assert_eq!(
        xdg.gog_credentials_lock(),
        Path::new("/home/me/.local/state/wineo/.locks/gog-credentials.lock")
    );
    assert_eq!(
        xdg.restore_marker("hades"),
        Path::new("/home/me/.local/state/wineo/.locks/snapshot-restore-hades-restore.json")
    );
    assert_eq!(
        one.restore_marker("hades"),
        Path::new("/mnt/a/wineo/prefixes/.wineo-snapshots/hades/restore.json")
    );
}

#[test]
fn config_paths_resolve_from_the_root_only_in_portable_mode() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    assert_eq!(
        paths.config_path("prefixes/x").unwrap(),
        Path::new("/mnt/usb/wineo/prefixes/x")
    );
    assert_eq!(paths.config_path("~/x").unwrap(), Path::new("/home/me/x"));
    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    assert_eq!(
        xdg.config_path("some/dir").unwrap(),
        xdg.absolute("some/dir").unwrap()
    );
}

#[test]
fn target_values_only_change_inside_a_portable_folder() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    let store = |value| paths.store_value(value).unwrap();
    assert_eq!(store("/mnt/usb/wineo/games/x.exe"), "games/x.exe");
    assert_eq!(store("/opt/x.exe"), "/opt/x.exe");
    assert_eq!(store("~/Games/x.exe"), "~/Games/x.exe");
    assert_eq!(store("C:\\Games\\x.exe"), "C:\\Games\\x.exe");
    assert_eq!(store("notepad"), "notepad");
    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    assert_eq!(
        xdg.store_value("/home/me/Games/x.exe").unwrap(),
        "/home/me/Games/x.exe"
    );
}

#[test]
fn portable_prefixes_must_be_inside_its_prefixes_dir() {
    let paths = portable(&[("HOME", "/home/me")], Some("/mnt/usb/wineo")).unwrap();
    assert!(
        paths
            .check_prefix(Path::new("/mnt/usb/wineo/prefixes/x"))
            .is_ok()
    );
    for outside in [
        "/mnt/usb/wineo/prefixes",
        "/mnt/usb/wineo/prefixes/.wineo-snapshots/x",
        "/mnt/usb/wineo/config/x",
        "/mnt/usb/wineo/prefixes/a/b",
        "/mnt/usb/wineo/prefixes/../x",
        "/home/me/prefix",
    ] {
        assert!(
            matches!(
                paths.check_prefix(Path::new(outside)),
                Err(Error::PrefixOutsidePortable(_))
            ),
            "{outside}"
        );
    }
    assert_eq!(
        paths
            .store_prefix(Path::new("/mnt/usb/wineo/prefixes/x"))
            .unwrap(),
        "prefixes/x"
    );
    let xdg = portable(&[("HOME", "/home/me")], None).unwrap();
    assert!(xdg.check_prefix(Path::new("/anywhere")).is_ok());

    assert!(
        paths
            .check_prefix_root(Path::new("/mnt/usb/wineo/prefixes"))
            .is_ok()
    );
    assert!(matches!(
        paths.check_prefix_root(Path::new("/mnt/usb/wineo/other")),
        Err(Error::PortablePrefixRoot(_))
    ));
    assert!(xdg.check_prefix_root(Path::new("/anywhere")).is_ok());
    assert_eq!(paths.shown("prefixes/x"), "/mnt/usb/wineo/prefixes/x");
    assert_eq!(xdg.shown("~/x"), "~/x");
}

#[test]
fn absolute_resolves_relative_paths() {
    let paths = paths(&[("HOME", "/home/me")]).unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(paths.absolute("some/dir").unwrap(), cwd.join("some/dir"));
    assert_eq!(paths.absolute("~/x").unwrap(), Path::new("/home/me/x"));
}
