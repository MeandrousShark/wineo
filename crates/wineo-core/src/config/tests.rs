use std::fs;

use super::*;

const PATH: &str = "/test/config.toml";

fn global(text: &str) -> Result<GlobalConfig> {
    GlobalConfig::parse(Path::new(PATH), text)
}

fn environment(text: &str) -> Result<EnvironmentConfig> {
    EnvironmentConfig::parse(Path::new(PATH), text)
}

#[test]
fn global_steam_picks_an_install() {
    assert_eq!(global("").unwrap().steam, None);
    assert_eq!(
        global("steam = \"flatpak\"\n").unwrap().steam,
        Some(crate::steam::InstallKind::Flatpak)
    );
    assert_eq!(
        global("steam = \"native\"\n").unwrap().steam,
        Some(crate::steam::InstallKind::Native)
    );
    assert!(global("steam = \"snap\"\n").is_err());
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
fn games_root_defaults_to_games_in_home_and_follows_the_config() {
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| "/home/me".into())).unwrap();
    let unset = global("").unwrap();
    assert_eq!(unset.defaults.games_root, None);
    assert_eq!(
        unset.games_root(&paths).unwrap(),
        Path::new("/home/me/Games")
    );
    let set = global("[defaults]\ngames_root = \"~/Library\"\n").unwrap();
    assert_eq!(
        set.games_root(&paths).unwrap(),
        Path::new("/home/me/Library")
    );
    let absolute = global("[defaults]\ngames_root = \"/mnt/games\"\n").unwrap();
    assert_eq!(
        absolute.games_root(&paths).unwrap(),
        Path::new("/mnt/games")
    );
    let relative = global("[defaults]\ngames_root = \"games\"\n").unwrap();
    assert_eq!(
        relative.games_root(&paths).unwrap(),
        paths.absolute("games").unwrap()
    );
}

#[test]
fn gog_language_defaults_to_us_english() {
    let unset = global("").unwrap();
    assert_eq!(unset.gog_language(), "en-US");
    assert_eq!(unset.gog_installer_language(), "en");
    let empty = global("[gog]\n").unwrap();
    assert_eq!(empty.gog_language(), "en-US");
    let set = global("[gog]\nlanguage = \"pt-br\"\n").unwrap();
    assert_eq!(set.gog_language(), "pt-br");
    assert_eq!(set.gog_installer_language(), "br");
}

#[test]
fn a_bad_gog_language_fails_the_global_config() {
    let err = global("[gog]\nlanguage = \"english\"\n").unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid config /test/config.toml: Invalid gog.language: use a locale tag like en-US."
    );
    assert!(global("[gog]\nlanguag = \"en\"\n").is_err());
}

#[test]
fn art_fetch_defaults_to_all() {
    assert_eq!(global("").unwrap().art.fetch, ArtFetch::All);
    assert_eq!(global("[art]").unwrap().art.fetch, ArtFetch::All);
}

#[test]
fn art_fetch_can_narrow_to_the_icon() {
    let config = global("[art]\nfetch = \"icon\"").unwrap();
    assert_eq!(config.art.fetch, ArtFetch::Icon);
    assert_eq!(
        global("[art]\nfetch = \"all\"").unwrap().art.fetch,
        ArtFetch::All
    );
}

#[test]
fn art_fetch_accepts_custom_and_empty_selections() {
    use crate::art::Kind;
    let fetch = global("[art]\nfetch = [\"grid\", \"logo\"]")
        .unwrap()
        .art
        .fetch;
    assert_eq!(fetch.kinds(), vec![Kind::Grid, Kind::Logo]);
    assert_eq!(fetch.to_string(), "grid,logo");
    assert_eq!(fetch.with(Kind::Grid, false).kinds(), vec![Kind::Logo]);
    assert!(
        global("[art]\nfetch = []")
            .unwrap()
            .art
            .fetch
            .kinds()
            .is_empty()
    );
    for raw in ["[\"grid\", \"grid\"]", "[\"wrong\"]", "[\"all\"]"] {
        assert!(global(&format!("[art]\nfetch = {raw}")).is_err());
    }
}

#[test]
fn unknown_art_fetch_values_and_keys_are_errors() {
    assert!(matches!(
        global("[art]\nfetch = \"grid\""),
        Err(Error::ConfigSyntax { .. })
    ));
    assert!(matches!(
        global("[art]\nfecth = \"icon\""),
        Err(Error::ConfigSyntax { .. })
    ));
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
    assert_eq!(config.wine().unwrap().prefix, "~/Apps/Photoshop");
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
fn heroic_id_is_optional_and_round_trips() {
    let config = environment("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"").unwrap();
    assert_eq!(config.heroic_id, None);
    let config =
        environment("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\nheroic_id = \"abc123\"")
            .unwrap();
    assert_eq!(config.heroic_id.as_deref(), Some("abc123"));
}

#[test]
fn gog_id_is_optional_and_round_trips_for_wine_and_native() {
    let wine = "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n";
    assert_eq!(environment(wine).unwrap().gog_id, None);
    for base in [wine, "kind = \"native\"\n"] {
        let config = environment(&format!("{base}gog_id = \"1094900565\"")).unwrap();
        assert_eq!(config.gog_id.as_deref(), Some("1094900565"));
    }
}

#[test]
fn gog_id_must_be_digits() {
    for (value, message) in [
        ("\"\"", "Invalid gog_id: empty."),
        ("\"12a\"", "Invalid gog_id: 12a isn't a number."),
        ("\"-1\"", "Invalid gog_id: -1 isn't a number."),
        ("\" 12\"", "Invalid gog_id:  12 isn't a number."),
        ("\"0123\"", "Invalid gog_id: 0123 isn't a GOG id."),
        (
            "\"18446744073709551616\"",
            "Invalid gog_id: 18446744073709551616 isn't a GOG id.",
        ),
    ] {
        let err = named(
            "rimworld",
            &format!("kind = \"native\"\ngog_id = {value}\n"),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("invalid config /test/rimworld.toml: {message}")
        );
    }
    assert!(environment("kind = \"native\"\ngog_id = 12\n").is_err());
}

#[test]
fn tags_parse_for_wine_and_native_environments() {
    let wine =
        environment("prefix = '/p'\nrunner = 'wine'\narch = 'win64'\ntags = ['game', 'co-op']\n")
            .unwrap();
    assert_eq!(wine.tags, ["game", "co-op"]);
    assert!(
        environment("prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n")
            .unwrap()
            .tags
            .is_empty()
    );
    let native = environment("kind = 'native'\ntags = ['work']\n").unwrap();
    assert_eq!(native.tags, ["work"]);
}

#[test]
fn winetricks_is_wine_only_and_validated() {
    let base = "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n";
    let config = environment(&format!("{base}winetricks = ['vcrun2022', 'win10']\n")).unwrap();
    assert_eq!(config.winetricks, ["vcrun2022", "win10"]);
    assert!(environment(base).unwrap().winetricks.is_empty());
    for value in ["['a', 'a']", "['two words']", "['-q']", "[1]", "'win10'"] {
        assert!(
            environment(&format!("{base}winetricks = {value}\n")).is_err(),
            "{value}"
        );
    }
    for value in ["[]", "['win10']"] {
        let err = environment(&format!("kind = 'native'\nwinetricks = {value}\n")).unwrap_err();
        assert!(err.to_string().contains("can't set winetricks"), "{err}");
    }
}

#[test]
fn tags_reject_bad_characters_and_duplicates() {
    for (tags, expected) in [
        ("['co op']", "Invalid tag \"co op\": spaces not allowed."),
        ("['game!']", "Invalid tag \"game!\": `!` not allowed."),
        ("['']", "Invalid tag \"\": can't be empty."),
        ("['game', 'game']", "Duplicate tag \"game\"."),
    ] {
        let err = environment(&format!("kind = 'native'\ntags = {tags}\n")).unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
    assert!(matches!(
        environment("kind = 'native'\ntags = [1]\n"),
        Err(Error::ConfigSyntax { .. })
    ));
}

#[test]
fn environment_requires_prefix_runner_and_arch() {
    assert!(environment("runner = \"wine\"\narch = \"win64\"").is_err());
    assert!(environment("prefix = \"/p\"\narch = \"win64\"").is_err());
    assert!(environment("prefix = \"/p\"\nrunner = \"wine\"").is_err());
}

const NATIVE: &str = r#"
        schema_version = 1
        kind = "native"
        gamemode = true

        [env]
        SDL_VIDEODRIVER = "wayland"

        [targets.game]
        exe = "/usr/bin/dusklight"
        default = true
        desktop = true
        name = "Dusklight"
    "#;

fn named(name: &str, text: &str) -> Result<EnvironmentConfig> {
    EnvironmentConfig::parse(Path::new(&format!("/test/{name}.toml")), text)
}

#[test]
fn a_native_config_parses() {
    let config = environment(NATIVE).unwrap();
    assert!(matches!(config.kind, Kind::Native));
    assert!(config.wine().is_none());
    assert_eq!(config.gamemode, Some(true));
    assert_eq!(config.env["SDL_VIDEODRIVER"], "wayland");
    assert_eq!(config.targets["game"].exe, "/usr/bin/dusklight");
}

#[test]
fn a_native_config_needs_no_targets() {
    assert!(matches!(
        environment("kind = \"native\"").unwrap().kind,
        Kind::Native
    ));
}

#[test]
fn each_wine_only_key_is_refused_for_native() {
    for (key, value) in [
        ("prefix", "\"/p\""),
        ("runner", "\"wine\""),
        ("arch", "\"win64\""),
        ("gameid", "\"umu-1\""),
        ("store", "\"gog\""),
        ("dxvk", "\"latest\""),
        ("vkd3d", "\"latest\""),
    ] {
        let err = named(
            "dusklight",
            &format!("kind = \"native\"\n{key} = {value}\n"),
        )
        .unwrap_err();
        assert!(matches!(err, Error::ConfigInvalid { .. }), "{key}: {err}");
        assert_eq!(
            err.to_string(),
            format!(
                "invalid config /test/dusklight.toml: native environment dusklight can't set {key}"
            )
        );
    }
}

#[test]
fn wayland_is_refused_for_native_at_either_level() {
    let err = named("dusklight", "kind = \"native\"\nwayland = false\n").unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid config /test/dusklight.toml: native environment dusklight can't set wayland"
    );
    let err = named(
        "dusklight",
        "kind = \"native\"\n[targets.game]\nexe = \"dusk\"\nwayland = true\n",
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid config /test/dusklight.toml: native environment dusklight can't set targets.game.wayland"
    );
}

#[test]
fn gamescope_and_mangohud_work_for_native() {
    let config = environment(
            "kind = \"native\"\nmangohud = true\n[gamescope]\nwidth = 1280\n[targets.game]\nexe = \"dusk\"\nmangohud = false\ngamescope = false\n",
        )
        .unwrap();
    assert_eq!(config.mangohud, Some(true));
    assert_eq!(
        config
            .gamescope
            .as_ref()
            .and_then(GamescopeSetting::on)
            .unwrap()
            .width,
        Some(1280)
    );
    let game = &config.targets["game"];
    assert_eq!(game.mangohud, Some(false));
    assert_eq!(game.gamescope, Some(GamescopeSetting::Off));
}

#[test]
fn launch_settings_parse_at_every_level() {
    let config = global("wayland = true\nmangohud = false\n").unwrap();
    assert_eq!(config.wayland, Some(true));
    assert_eq!(config.mangohud, Some(false));
    let config = environment(&format!(
            "{ENV}wayland = true\nmangohud = true\n[gamescope]\nfilter = \"fsr\"\n[targets.game]\nexe = \"g\"\nwayland = false\n[targets.game.gamescope]\nwindow = \"borderless\"\n"
        ))
        .unwrap();
    assert_eq!(config.wayland, Some(true));
    assert_eq!(config.mangohud, Some(true));
    assert!(config.gamescope.as_ref().unwrap().on().is_some());
    let game = &config.targets["game"];
    assert_eq!(game.wayland, Some(false));
    assert_eq!(game.mangohud, None);
    let table = game.gamescope.as_ref().unwrap().on().unwrap();
    assert_eq!(table.window, Some(crate::gamescope::Window::Borderless));
    assert_eq!(table.filter, None);
}

#[test]
fn gamescope_is_not_a_global_key() {
    assert!(global("[gamescope]\nwidth = 1\n").is_err());
}

#[test]
fn gamescope_true_and_bad_values_are_refused_at_load() {
    for text in [
        "gamescope = true\n",
        "[gamescope]\nfilter = \"bilinear\"\n",
        "[gamescope]\nscaler = \"zoom\"\n",
        "[gamescope]\nwindow = \"windowed\"\n",
        "[targets.game]\nexe = \"g\"\ngamescope = true\n",
        "[targets.game]\nexe = \"g\"\n[targets.game.gamescope]\nscaler = \"big\"\n",
    ] {
        let err = environment(&format!("{ENV}{text}")).unwrap_err();
        assert!(matches!(err, Error::ConfigSyntax { .. }), "{text}: {err}");
    }
    let err = environment(&format!("{ENV}gamescope = true\n")).unwrap_err();
    assert!(
        format!("{err:?}").contains("gamescope takes a [gamescope] table or false, not true"),
        "{err:?}"
    );
}

#[test]
fn a_config_without_kind_is_wine() {
    let config = environment(ENV).unwrap();
    let wine = config.wine().unwrap();
    assert_eq!(wine.prefix, "/p");
    assert_eq!(wine.runner, "wine");
    assert_eq!(wine.arch, Arch::Win64);
}

#[test]
fn an_explicit_wine_kind_is_wine() {
    let config = environment(&format!("kind = \"wine\"\n{ENV}")).unwrap();
    assert_eq!(config.wine().unwrap().prefix, "/p");
}

#[test]
fn an_explicit_wine_kind_still_needs_prefix_runner_and_arch() {
    let err = environment("kind = \"wine\"\nrunner = \"wine\"\narch = \"win64\"").unwrap_err();
    assert!(err.to_string().contains("missing field `prefix`"), "{err}");
}

#[test]
fn an_unknown_kind_is_refused() {
    for kind in ["\"proton\"", "\"Native\"", "1"] {
        assert!(
            matches!(
                environment(&format!("kind = {kind}\n{ENV}")),
                Err(Error::ConfigSyntax { .. })
            ),
            "{kind}"
        );
    }
}

#[test]
fn unknown_keys_are_refused_for_native_too() {
    assert!(matches!(
        environment("kind = \"native\"\nprefx = 1"),
        Err(Error::ConfigSyntax { .. })
    ));
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
fn wineprefix_in_env_is_allowed_for_native() {
    let config = environment("kind = \"native\"\n[env]\nWINEPREFIX = \"/x\"").unwrap();
    assert_eq!(config.env["WINEPREFIX"], "/x");
    assert!(environment("kind = \"native\"\n[env]\n\"A=B\" = \"1\"").is_err());
}

#[test]
fn env_keys_are_checked_like_the_config_checks_them() {
    assert!(validate_env_key("WINEPREFIX", true).is_err());
    assert!(validate_env_key("WINEPREFIX", false).is_ok());
    assert!(validate_env_key("A=B", false).is_err());
    assert!(validate_env_key("", true).is_err());
    assert!(validate_env_key("DXVK_HUD", true).is_ok());
}

#[test]
fn wineprefix_is_reported_before_other_bad_names() {
    let err = environment(
            "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n[env]\n\"A=B\" = \"1\"\nWINEPREFIX = \"/x\"",
        )
        .unwrap_err();
    assert!(err.to_string().contains("WINEPREFIX is managed"), "{err}");
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

const ENV: &str = "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n";

#[test]
fn a_missing_schema_version_is_legacy() {
    assert_eq!(global("").unwrap().schema_version, 0);
    assert_eq!(environment(ENV).unwrap().schema_version, 0);
}

#[test]
fn schema_version_one_parses() {
    assert_eq!(global("schema_version = 1").unwrap().schema_version, 1);
    let config = environment(&format!("schema_version = 1\n{ENV}")).unwrap();
    assert_eq!(config.schema_version, 1);
}

#[test]
fn unsupported_schema_versions_are_rejected() {
    for (value, found) in [
        ("0", "0"),
        ("2", "2"),
        ("-1", "-1"),
        ("\"1\"", "\"1\""),
        ("1.0", "1.0"),
        ("true", "true"),
    ] {
        let text = format!("schema_version = {value}\n");
        let err = global(&text).unwrap_err();
        assert!(matches!(err, Error::SchemaVersion { .. }), "{value}: {err}");
        assert_eq!(
            err.to_string(),
            format!("unsupported schema_version {found} in {PATH}")
        );
        let err = environment(&format!("{text}{ENV}")).unwrap_err();
        assert!(matches!(err, Error::SchemaVersion { .. }), "{value}: {err}");
    }
}

// A newer format may add keys, and the version is what should be reported.
#[test]
fn a_newer_version_is_reported_before_unknown_keys() {
    let err = environment(&format!("schema_version = 2\n{ENV}new_key = 1\n")).unwrap_err();
    assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
}

#[test]
fn a_nested_schema_version_is_an_unknown_key() {
    assert!(matches!(
        global("[defaults]\nschema_version = 1"),
        Err(Error::ConfigSyntax { .. })
    ));
}

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| {
        (key == "HOME").then(|| std::ffi::OsString::from(home.as_os_str()))
    })
    .unwrap();
    (root, paths)
}

fn write_global(paths: &Paths, text: &str) {
    let path = paths.global_config();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn a_missing_global_config_stays_missing() {
    let (_root, paths) = temp_paths();
    assert_eq!(GlobalConfig::load(&paths).unwrap().schema_version, 0);
    assert_eq!(
        GlobalConfig::load_or_default(&paths)
            .unwrap()
            .schema_version,
        0
    );
    assert!(!paths.global_config().exists());
}

#[test]
fn loading_a_legacy_global_config_leaves_it_alone() {
    let (_root, paths) = temp_paths();
    let text = "# mine\nstop_timeout = 3\n";
    write_global(&paths, text);
    let path = paths.global_config();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(GlobalConfig::load(&paths).unwrap().stop_timeout, 3);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn load_or_default_only_forgives_a_broken_file() {
    let (_root, paths) = temp_paths();
    write_global(&paths, "stop_timeout = [\n");
    let config = GlobalConfig::load_or_default(&paths).unwrap();
    assert_eq!(config.stop_timeout, DEFAULT_STOP_TIMEOUT);

    for version in ["0", "2", "\"1\""] {
        write_global(
            &paths,
            &format!("schema_version = {version}\nstop_timeout = 3\n"),
        );
        assert!(matches!(
            GlobalConfig::load_or_default(&paths),
            Err(Error::SchemaVersion { .. })
        ));
    }
}

#[test]
fn stamping_keeps_a_leading_comment_on_top() {
    let text = "# mine\nstop_timeout = 3 # short\n\n[defaults]\nrunner = \"x\"\n";
    let mut doc: DocumentMut = text.parse().unwrap();
    stamp_schema(&mut doc);
    assert_eq!(
        doc.to_string(),
        "# mine\nstop_timeout = 3 # short\nschema_version = 1\n\n[defaults]\nrunner = \"x\"\n"
    );
    let stamped = doc.to_string();
    stamp_schema(&mut doc);
    assert_eq!(doc.to_string(), stamped);
}

#[test]
fn a_new_document_starts_with_the_version() {
    let mut doc = new_document();
    doc["prefix"] = value("/p");
    assert_eq!(doc.to_string(), "schema_version = 1\nprefix = \"/p\"\n");
}

const EXAMPLE: &str = include_str!("../../../../docs/environment.toml.example");

/// A commented-out setting is a line `# key = value` or `# [table]`; a
/// prose comment never starts a key right after `# `, so this tells the
/// two apart without having to track table context.
fn is_example_setting(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("# ") else {
        return false;
    };
    if rest.starts_with('[') {
        return true;
    }
    let key_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .unwrap_or(rest.len());
    key_end > 0 && rest[key_end..].trim_start().starts_with('=')
}

const NATIVE_EXAMPLE: &str = include_str!("../../../../docs/native-environment.toml.example");

/// Uncomments every setting line in an example, leaving prose comments
/// and blank lines alone.
fn uncommented_example(example: &str) -> String {
    example
        .lines()
        .map(|line| {
            if is_example_setting(line) {
                &line[2..]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn example_config_parses_as_shipped() {
    let config = environment(EXAMPLE).unwrap();
    assert_eq!(config.schema_version, 1);
    assert_eq!(config.wine().unwrap().prefix, "[PREFIX-DIR]");
    assert_eq!(config.wine().unwrap().runner, "GE-Proton-Latest");
    assert_eq!(config.wine().unwrap().arch, Arch::Win64);
    assert!(config.gamemode.is_none());
    assert!(config.mangohud.is_none());
    assert!(config.wayland.is_none());
    assert!(config.gamescope.is_none());
    assert!(config.wine().unwrap().gameid.is_none());
    assert!(config.wine().unwrap().store.is_none());
    assert!(config.wine().unwrap().dxvk.is_none());
    assert!(config.wine().unwrap().vkd3d.is_none());
    assert!(config.heroic_id.is_none());
    assert!(config.env.is_empty());
    assert!(config.targets.is_empty());
}

/// Every optional field of `EnvironmentConfig` and `Target` must have an
/// example line, so a new field with no line here fails this test.
#[test]
fn example_config_parses_with_every_setting_uncommented() {
    let text = uncommented_example(EXAMPLE);
    let config = environment(&text)
        .unwrap_or_else(|err| panic!("uncommented example did not parse: {err}\n---\n{text}"));

    assert!(config.gamemode.unwrap());
    assert!(config.mangohud.unwrap());
    assert!(config.wayland.unwrap());
    let gamescope = config
        .gamescope
        .as_ref()
        .and_then(GamescopeSetting::on)
        .unwrap();
    assert_eq!(gamescope.args(false).last().unwrap(), "--hdr-enabled");
    assert!(gamescope.grab_cursor);
    assert!(config.wine().unwrap().gameid.is_some());
    assert!(config.wine().unwrap().store.is_some());
    assert!(config.wine().unwrap().dxvk.is_some());
    assert!(config.wine().unwrap().vkd3d.is_some());
    assert!(config.heroic_id.is_some());
    assert!(!config.env.is_empty());
    assert_eq!(config.env["DXVK_HUD"], "1");

    let game = &config.targets["game"];
    assert!(!game.exe.is_empty());
    assert!(!game.args.is_empty());
    assert!(game.cwd.is_some());
    assert!(game.default);
    assert!(game.gamemode.unwrap());
    assert!(game.mangohud.unwrap());
    assert!(game.wayland.unwrap());
    assert!(game.desktop);
    assert!(game.name.is_some());

    let launcher = &config.targets["launcher"];
    assert!(!launcher.exe.is_empty());
    assert_eq!(launcher.gamescope, Some(GamescopeSetting::Off));
}

#[test]
fn native_example_config_parses_as_shipped() {
    let config = environment(NATIVE_EXAMPLE).unwrap();
    assert_eq!(config.schema_version, 1);
    assert!(matches!(config.kind, Kind::Native));
    assert!(config.gamemode.is_none());
    assert!(config.mangohud.is_none());
    assert!(config.gamescope.is_none());
    assert!(config.heroic_id.is_none());
    assert!(config.env.is_empty());
    assert!(config.targets.is_empty());
}

/// Like the Wine example, every optional field a native config can hold
/// must have an example line.
#[test]
fn native_example_config_parses_with_every_setting_uncommented() {
    let text = uncommented_example(NATIVE_EXAMPLE);
    let config = environment(&text)
        .unwrap_or_else(|err| panic!("uncommented example did not parse: {err}\n---\n{text}"));

    assert!(matches!(config.kind, Kind::Native));
    assert!(config.gamemode.unwrap());
    assert!(config.mangohud.unwrap());
    assert!(
        config
            .gamescope
            .as_ref()
            .and_then(GamescopeSetting::on)
            .is_some()
    );
    assert!(config.heroic_id.is_some());
    assert!(!config.env.is_empty());

    let game = &config.targets["game"];
    assert!(!game.exe.is_empty());
    assert!(!game.args.is_empty());
    assert!(game.cwd.is_some());
    assert!(game.default);
    assert!(game.gamemode.unwrap());
    assert!(game.mangohud.unwrap());
    assert!(game.desktop);
    assert!(game.name.is_some());

    let dusklight = &config.targets["dusklight"];
    assert_eq!(dusklight.exe, "dusklight");
    assert_eq!(dusklight.gamescope, Some(GamescopeSetting::Off));
}

const PROXIED_EXAMPLE: &str = include_str!("../../../../docs/proxied-environment.toml.example");

#[test]
fn proxied_example_config_parses_as_shipped() {
    let config = environment(PROXIED_EXAMPLE).unwrap();
    assert_eq!(config.schema_version, 1);
    let proxied = config.proxied().unwrap();
    assert_eq!(proxied.launcher, Launcher::Steam);
    assert_eq!(proxied.app_id, "1086940");
    assert!(proxied.title.is_none());
    assert!(!proxied.desktop);
    assert!(config.tags.is_empty());
    assert!(config.hooks.is_none());
    assert!(config.wine().is_none());
    assert!(!config.is_native());
}

/// Every optional field a proxied config can hold must have an example
/// line.
#[test]
fn proxied_example_config_parses_with_every_setting_uncommented() {
    let text = uncommented_example(PROXIED_EXAMPLE);
    let config = environment(&text)
        .unwrap_or_else(|err| panic!("uncommented example did not parse: {err}\n---\n{text}"));
    let proxied = config.proxied().unwrap();
    assert!(proxied.title.is_some());
    assert!(proxied.desktop);
    assert!(!config.tags.is_empty());
    let hooks = config.hooks.as_ref().and_then(HooksSetting::on).unwrap();
    assert!(hooks.before.is_some());
    assert!(hooks.after.is_some());
}

const PROXIED: &str = "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\n";

#[test]
fn a_proxied_config_parses() {
    let config = named(
            "bg3",
            &format!(
                "schema_version = 1\n{PROXIED}title = \"Baldur's Gate 3\"\ntags = [\"game\"]\ndesktop = true\n[hooks]\nafter = \"true\"\n"
            ),
        )
        .unwrap();
    assert_eq!(config.kind.as_str(), "proxied");
    assert_eq!(
        config.proxied(),
        Some(&ProxiedConfig {
            launcher: Launcher::Steam,
            app_id: "1086940".to_owned(),
            title: Some("Baldur's Gate 3".to_owned()),
            desktop: true,
        })
    );
    assert_eq!(config.tags, ["game"]);
    assert!(config.targets.is_empty());
    assert!(config.env.is_empty());
    assert!(config.gamemode.is_none());
}

#[test]
fn a_proxied_config_needs_a_launcher_and_an_app_id() {
    let err = named("bg3", "kind = \"proxied\"\napp_id = \"1\"\n").unwrap_err();
    assert!(
        err.to_string().ends_with("missing field `launcher`"),
        "{err}"
    );
    let err = named("bg3", "kind = \"proxied\"\nlauncher = \"steam\"\n").unwrap_err();
    assert!(err.to_string().ends_with("missing field `app_id`"), "{err}");
    assert!(matches!(
        named(
            "bg3",
            "kind = \"proxied\"\nlauncher = \"heroic\"\napp_id = \"1\"\n"
        ),
        Err(Error::ConfigSyntax { .. })
    ));
}

#[test]
fn a_proxied_app_id_is_not_empty_and_has_no_nul() {
    for (app_id, message) in [
        ("\"\"", "Invalid app_id: empty."),
        ("\"1\\u0000\"", "Invalid app_id: contains a NUL byte."),
    ] {
        let err = named(
            "bg3",
            &format!("kind = \"proxied\"\nlauncher = \"steam\"\napp_id = {app_id}\n"),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("invalid config /test/bg3.toml: {message}")
        );
    }
}

#[test]
fn each_refused_key_is_refused_for_proxied() {
    for (key, line) in [
        ("prefix", "prefix = \"/p\""),
        ("runner", "runner = \"wine\""),
        ("arch", "arch = \"win64\""),
        ("gameid", "gameid = \"umu-1\""),
        ("store", "store = \"gog\""),
        ("dxvk", "dxvk = \"latest\""),
        ("vkd3d", "vkd3d = \"latest\""),
        ("wayland", "wayland = true"),
        ("winetricks", "winetricks = []"),
        ("gamemode", "gamemode = true"),
        ("mangohud", "mangohud = false"),
        ("gamescope", "gamescope = false"),
        ("heroic_id", "heroic_id = \"abc\""),
        ("gog_id", "gog_id = \"1094900565\""),
        ("env", "[env]"),
        ("targets", "[targets.game]\nexe = \"/bin/true\""),
    ] {
        let err = named("bg3", &format!("{PROXIED}{line}\n")).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("invalid config /test/bg3.toml: bg3 is a Steam game, so it can't set {key}."),
            "{key}"
        );
    }
}

#[test]
fn proxied_only_keys_are_refused_for_wine_and_native() {
    let wine = "prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n";
    let explicit_wine = format!("kind = \"wine\"\n{wine}");
    for kind in [wine, "kind = \"native\"\n", explicit_wine.as_str()] {
        for (key, line) in [
            ("launcher", "launcher = \"steam\""),
            ("app_id", "app_id = \"1\""),
            ("title", "title = \"Dusk\""),
            ("desktop", "desktop = false"),
        ] {
            let err = named("dusk", &format!("{kind}{line}\n")).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "invalid config /test/dusk.toml: dusk isn't a Steam game, so it can't set {key}."
                ),
                "{kind}{key}"
            );
        }
    }
}

#[test]
fn no_prefix_names_the_owner() {
    let native = named("dusk", "kind = \"native\"\n").unwrap();
    assert_eq!(
        native.require_wine("dusk").unwrap_err().to_string(),
        "dusk is a native environment, so it has no prefix"
    );
    let proxied = named("bg3", PROXIED).unwrap();
    assert_eq!(
        proxied.require_wine("bg3").unwrap_err().to_string(),
        "Steam manages bg3's prefix."
    );
    assert!(matches!(
        proxied.no_prefix("bg3"),
        Error::NoPrefix(NoPrefix {
            owner: PrefixOwner::Launcher(Launcher::Steam),
            ..
        })
    ));
}

#[test]
fn arch_round_trips_through_strings() {
    assert_eq!("win64".parse::<Arch>().unwrap(), Arch::Win64);
    assert_eq!("win32".parse::<Arch>().unwrap(), Arch::Win32);
    assert!("x86".parse::<Arch>().is_err());
    assert_eq!(Arch::Win32.to_string(), "win32");
}

#[test]
fn hooks_parse_and_resolve_for_wine_and_native() {
    for base in [ENV, "kind = 'native'\n"] {
        let text = format!(
            "{base}[hooks]\nbefore = 'mount /mnt/games'\nafter = 'sync'\n\
                 [targets.inherit]\nexe = 'game'\n\
                 [targets.own]\nexe = 'editor'\n[targets.own.hooks]\nafter = 'notify-send done'\n\
                 [targets.off]\nexe = 'tool'\nhooks = false\n\
                 [targets.empty]\nexe = 'tool'\n[targets.empty.hooks]\n"
        );
        let env = crate::environment::Environment {
            name: "game".to_owned(),
            path: "/x/game.toml".into(),
            config: environment(&text).unwrap(),
        };
        let resolve = |name| crate::launch::hooks(&env, env.config.targets.get(name));
        let inherited = resolve("inherit").unwrap();
        assert_eq!(inherited.before.as_deref(), Some("mount /mnt/games"));
        assert_eq!(inherited.after.as_deref(), Some("sync"));
        assert_eq!(crate::launch::hooks(&env, None), Some(inherited));
        let own = resolve("own").unwrap();
        assert_eq!(own.before, None);
        assert_eq!(own.after.as_deref(), Some("notify-send done"));
        assert_eq!(resolve("off"), None);
        assert_eq!(resolve("empty"), Some(&Hooks::default()));

        let bare = crate::environment::Environment {
            name: "bare".into(),
            path: "/x/bare.toml".into(),
            config: environment(base).unwrap(),
        };
        assert_eq!(crate::launch::hooks(&bare, None), None);
        let target_only = crate::environment::Environment {
                name: "only".into(),
                path: "/x/only.toml".into(),
                config: environment(&format!(
                    "{base}hooks = false\n[targets.game]\nexe = 'g'\n[targets.game.hooks]\nbefore = 'sync'\n"
                )).unwrap(),
            };
        assert_eq!(crate::launch::hooks(&target_only, None), None);
        assert_eq!(
            crate::launch::hooks(&target_only, target_only.config.targets.get("game"))
                .unwrap()
                .before
                .as_deref(),
            Some("sync")
        );
    }
    assert!(global("[hooks]\nbefore = 'sync'\n").is_err());
}

#[test]
fn hooks_reject_true_empty_commands_and_invalid_fields_at_load() {
    for base in [ENV, "kind = 'native'\n"] {
        for prefix in ["", "[targets.game]\nexe = 'g'\n"] {
            let err = environment(&format!("{base}{prefix}hooks = true\n")).unwrap_err();
            assert!(
                format!("{err:?}").contains("hooks takes a [hooks] table or false, not true"),
                "{err}"
            );
            let table = if prefix.is_empty() {
                "hooks"
            } else {
                "targets.game.hooks"
            };
            for field in ["before", "after"] {
                let err =
                    environment(&format!("{base}{prefix}[{table}]\n{field} = ''\n")).unwrap_err();
                assert!(
                    format!("{err:?}").contains(&format!("Invalid hooks.{field}: empty command.")),
                    "{err}"
                );
                assert!(
                    environment(&format!("{base}{prefix}[{table}]\n{field} = false\n")).is_err()
                );
            }
            assert!(environment(&format!("{base}{prefix}[{table}]\nunknown = 'sync'\n")).is_err());
        }
    }
}
