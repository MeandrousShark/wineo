use crate::test_script::write_script;
use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

fn fixture() -> (TempDir, HeroicSource) {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    let source = HeroicSource::from_config_dir_with_home(config, root.path().to_owned());
    (root, source)
}

fn write_fixture(source: &HeroicSource, global: &str, game: &str, library: &str) {
    fs::write(source.config_path(), global).unwrap();
    fs::write(source.library_path(), library).unwrap();
    if !game.is_empty() {
        fs::write(source.game_config_path("hades"), game).unwrap();
    }
}

fn wine_prefix(root: &Path) -> PathBuf {
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    prefix
}

fn game_fixture(source: &HeroicSource, root: &Path, settings: &str, native: bool) {
    let prefix = wine_prefix(root);
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let platform = if native { "linux" } else { "Windows" };
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"{platform}","executable":"{}"}}}}]}}"#,
        prefix.join("drive_c/game.exe").display()
    );
    write_fixture(
        source,
        &global,
        &format!(r#"{{"hades":{settings}}}"#),
        &library,
    );
}

#[test]
fn a_prefix_holding_other_prefixes_is_invalid() {
    let (root, source) = fixture();
    game_fixture(&source, root.path(), "{}", false);
    let other = root.path().join("prefix/Other Game");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let imported = source.inspect("hades").unwrap();
    let issue = imported
        .issues
        .iter()
        .find(|issue| issue.field.as_deref() == Some("winePrefix"))
        .unwrap();
    assert_eq!(issue.kind, HeroicIssueKind::Invalid);
    assert_eq!(
        issue.message,
        "prefix holds other prefixes; choose the game's own prefix"
    );
}

#[test]
fn gamescope_fields_follow_heroic_enable_flags_and_skip_bad_numbers() {
    let (root, source) = fixture();
    game_fixture(
        &source,
        root.path(),
        r#"{"showMangohud":true,"gamescope":{"enableUpscaling":true,"enableLimiter":true,"gameWidth":"1920","gameHeight":"1080","upscaleWidth":"2560","upscaleHeight":"1440","upscaleMethod":"fsr","windowType":"fullscreen","fpsLimiter":"60","fpsLimiterNoFocus":"10","enableForceGrabCursor":true,"additionalOptions":"--hdr-enabled --label 'two words'"}}"#,
        false,
    );
    let imported = source.inspect("hades").unwrap();
    assert_eq!(imported.mangohud, Some(true));
    let game = imported.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!(
        (
            game.width,
            game.height,
            game.output_width,
            game.output_height
        ),
        (Some(1920), Some(1080), Some(2560), Some(1440))
    );
    assert_eq!(
        (
            game.filter,
            game.window,
            game.fps_limit,
            game.fps_limit_unfocused
        ),
        (
            Some(Filter::Fsr),
            Some(Window::Fullscreen),
            Some(60),
            Some(10)
        )
    );
    assert!(game.grab_cursor);
    assert_eq!(game.args, ["--hdr-enabled", "--label", "two words"]);
    assert!(
        !imported
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("gamescope")
                || issue.field.as_deref() == Some("showMangohud"))
    );

    fs::write(source.game_config_path("hades"), r#"{"hades":{"showMangohud":true,"gamescope":{"enableUpscaling":false,"enableLimiter":false,"gameWidth":"1920"}}}"#).unwrap();
    let imported = source.inspect("hades").unwrap();
    assert_eq!(imported.mangohud, Some(true));
    assert_eq!(imported.gamescope, None);

    fs::write(source.game_config_path("hades"), r#"{"hades":{"gamescope":{"enableLimiter":true,"fpsLimiter":"","fpsLimiterNoFocus":"oops","gameWidth":"not used","enableForceGrabCursor":true}}}"#).unwrap();
    let imported = source.inspect("hades").unwrap();
    let game = imported.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!(game.fps_limit, None);
    assert_eq!(game.fps_limit_unfocused, None);
    assert_eq!(game.width, None);
    assert!(game.grab_cursor);
    assert_eq!(
        imported
            .issues
            .iter()
            .filter(|issue| issue.field.as_deref() == Some("gamescope.fpsLimiterNoFocus"))
            .count(),
        1
    );
}

#[test]
fn shell_path_quoting_keeps_heroic_paths_as_one_word() {
    let path = "script with 'quote'/$dollar;`tick`\nnext line";
    assert_eq!(
        quote_shell_path(path),
        "'script with '\\''quote'\\''/$dollar;`tick`\nnext line'"
    );
}

#[test]
fn a_quoted_heroic_script_path_runs_that_script() {
    use std::process::Command;

    let root = tempfile::tempdir().unwrap();
    let script = root
        .path()
        .join("script with 'quote' $dollar;`tick`\nnext line.sh");
    let marker = root.path().join("marker");
    write_script(
        &script,
        format!("#!/bin/sh\nprintf 'ran' > '{}'\n", marker.display()),
    );

    let status = Command::new("sh")
        .arg("-c")
        .arg(quote_shell_path(script.to_str().unwrap()))
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(fs::read_to_string(marker).unwrap(), "ran");
}

#[test]
fn heroic_wine_and_native_scripts_become_quoted_hooks_and_review_rows() {
    let (root, source) = fixture();
    let before = "/scripts/before $dollar;`tick` 'quoted'.sh";
    let after = "/scripts/after\nline.sh";
    let settings = serde_json::json!({
        "beforeLaunchScriptPath": before,
        "afterLaunchScriptPath": after,
    })
    .to_string();

    game_fixture(&source, root.path(), &settings, false);
    let wine = source.inspect("hades").unwrap();
    let hooks = wine.hooks.as_ref().unwrap().on().unwrap();
    assert_eq!(
        hooks.before.as_deref(),
        Some(quote_shell_path(before).as_str())
    );
    assert_eq!(
        hooks.after.as_deref(),
        Some(quote_shell_path(after).as_str())
    );
    assert!(
        wine.settings_review
            .carried
            .contains(&review_item("before hook", before))
    );
    assert!(
        wine.settings_review
            .carried
            .contains(&review_item("after hook", after))
    );
    assert!(!wine.issues.iter().any(|issue| {
        matches!(
            issue.field.as_deref(),
            Some("beforeLaunchScriptPath" | "afterLaunchScriptPath")
        )
    }));

    let native_settings = settings
        .strip_prefix('{')
        .unwrap()
        .strip_suffix('}')
        .unwrap();
    let (_root, source) = native_game(native_settings);
    let native = source.inspect_native("hades").unwrap();
    let hooks = native.hooks.as_ref().unwrap().on().unwrap();
    assert_eq!(
        hooks.before.as_deref(),
        Some(quote_shell_path(before).as_str())
    );
    assert_eq!(
        hooks.after.as_deref(),
        Some(quote_shell_path(after).as_str())
    );
    assert!(
        native
            .settings_review
            .carried
            .contains(&review_item("before hook", before))
    );
    assert!(
        native
            .settings_review
            .carried
            .contains(&review_item("after hook", after))
    );
}

#[test]
fn empty_heroic_script_paths_are_unset() {
    let (_root, source) =
        native_game(r#""beforeLaunchScriptPath":"  ","afterLaunchScriptPath":"""#);
    let imported = source.inspect_native("hades").unwrap();
    assert!(imported.hooks.is_none());
    assert!(
        !imported
            .settings_review
            .carried
            .iter()
            .any(|item| item.field.ends_with("hook"))
    );
}

#[test]
fn gamescope_alternatives_native_and_global_defaults() {
    let (root, source) = fixture();
    game_fixture(
        &source,
        root.path(),
        r#"{"enableWineWayland":true,"enableHDR":true,"showMangohud":true,"gamescope":{"enableUpscaling":true,"upscaleMethod":"integer","windowType":"borderless"}}"#,
        true,
    );
    let native = source.inspect_native("hades").unwrap();
    assert_eq!(native.mangohud, Some(true));
    let game = native.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!(
        (game.scaler, game.window),
        (Some(Scaler::Integer), Some(Window::Borderless))
    );
    assert!(!native.issues.iter().any(|issue| matches!(
        issue.field.as_deref(),
        Some("enableWineWayland" | "enableHDR" | "gamescope")
    )));

    let prefix = root.path().join("prefix");
    fs::write(source.config_path(), format!(r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"enableWineWayland":true,"showMangohud":true,"gamescope":{{"enableUpscaling":true,"upscaleMethod":"nis","windowType":"windowed","gameWidth":""}}}}}}"#, prefix.display())).unwrap();
    fs::write(source.library_path(), format!(r#"{{"games":[{{"app_name":"hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#, prefix.join("drive_c/game.exe").display())).unwrap();
    fs::write(source.game_config_path("hades"), r#"{"hades":{}}"#).unwrap();
    let imported = source.inspect("hades").unwrap();
    assert!(imported.enable_wine_wayland);
    assert_eq!(imported.mangohud, Some(true));
    let game = imported.gamescope.unwrap();
    let game = game.on().unwrap();
    assert_eq!(game.filter, Some(Filter::Nis));
    assert_eq!(game.window, None);
    assert_eq!(game.width, None);
    fs::write(source.game_config_path("hades"), r#"{"hades":{"gamescope":{"enableUpscaling":true,"upscaleMethod":"stretch"},"showMangohud":false,"enableWineWayland":false}}"#).unwrap();
    let imported = source.inspect("hades").unwrap();
    assert!(!imported.enable_wine_wayland);
    assert_eq!(imported.mangohud, Some(false));
    assert_eq!(imported.wayland, Some(false));
    assert!(
        imported
            .settings_review
            .carried
            .iter()
            .any(|item| item.field == "showMangohud" && item.detail == "false")
    );
    assert_eq!(
        imported.gamescope.unwrap().on().unwrap().scaler,
        Some(Scaler::Stretch)
    );
}

#[test]
fn hdr_without_wayland_remains_unsupported() {
    let (root, source) = fixture();
    game_fixture(
        &source,
        root.path(),
        r#"{"enableHDR":true,"enableWineWayland":false,"enableWoW64":true}"#,
        false,
    );
    let imported = source.inspect("hades").unwrap();
    assert!(!imported.enable_wine_wayland);
    assert!(imported.enable_hdr);
    assert!(
        imported
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("enableHDR")
                && issue.kind == HeroicIssueKind::Unsupported)
    );
    assert!(
        imported
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("enableWoW64")
                && issue.message.contains("runner's build decides WoW64"))
    );
}

#[test]
fn effective_settings_preserve_false_empty_arrays_and_shell_boundaries() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"version":"v0","defaultSettings":{{
                "winePrefix":"{}",
                "wineVersion":{{"type":"wine","name":"System","bin":"wine"}},
                "enviromentOptions":[{{"key":"GLOBAL","value":"global"}},{{"key":"EMPTY","value":"global"}}],
                "enableFSR":true,"enableEsync":true,"useGameMode":true
            }}}}"#,
        prefix.display()
    );
    let game = r#"{
            "version":"v0.1",
            "hades":{"enviromentOptions":[],"enableFSR":false,"enableEsync":false,"useGameMode":false,"launcherArgs":"--name 'two words'"}
        }"#;
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("drive_c/Hades.exe").display()
    );
    write_fixture(&source, &global, game, &library);
    let import = source.inspect("hades").unwrap();
    assert_eq!(import.args, ["--name", "two words"]);
    assert!(!import.env.contains_key("GLOBAL"));
    assert_eq!(import.env.get("WINE_FULLSCREEN_FSR"), Some(&"0".to_owned()));
    assert_eq!(import.gamemode, Some(false));
    assert!(
        import
            .fingerprint
            .files
            .iter()
            .all(|file| file.digest.is_some())
    );
}

#[test]
fn list_isolates_invalid_and_native_entries() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[null,{{"app_name":"../bad","title":"Bad","runner":"sideload","install":{{"platform":"Windows","executable":"x.exe"}}}},{{"app_name":"native","title":"Native","runner":"sideload","install":{{"platform":"Linux","executable":"x"}}}},{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("game.exe").display()
    );
    write_fixture(&source, &global, "", &library);
    let listing = source.list_candidates().unwrap();
    assert_eq!(listing.candidates.len(), 3);
    assert!(!listing.issues.is_empty());
    assert!(
        listing
            .candidates
            .iter()
            .any(|candidate| candidate.id == "native" && !candidate.issues.is_empty())
    );
    assert!(
        listing
            .candidates
            .iter()
            .any(|candidate| candidate.id == "hades" && candidate.issues.is_empty())
    );
}

#[test]
fn conflicting_managed_environment_is_reported_without_being_imported() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"enviromentOptions":[{{"key":"WINEPREFIX","value":"/tmp/other"}},{{"key":"KEEP","value":""}}]}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("game.exe").display()
    );
    write_fixture(&source, &global, "", &library);
    let import = source.inspect("hades").unwrap();
    assert!(!import.env.contains_key("WINEPREFIX"));
    assert_eq!(import.env.get("KEEP"), Some(&String::new()));
    assert!(
        import
            .issues
            .iter()
            .any(|issue| issue.kind == HeroicIssueKind::Conflict)
    );
}

/// A native sideload `hades`, with `settings` as Heroic's defaults.
fn native_game(settings: &str) -> (TempDir, HeroicSource) {
    let (root, source) = fixture();
    let global = format!(r#"{{"defaultSettings":{{{settings}}}}}"#);
    let library = r#"{"games":[{"app_name":"hades","title":"Hades","runner":"sideload","install":{"platform":"linux","executable":"/usr/bin/hades"}}]}"#;
    write_fixture(&source, &global, "", library);
    (root, source)
}

#[test]
fn a_native_import_carries_nvidia_prime_under_explicit_variables() {
    let (_root, source) = native_game(
        r#""nvidiaPrime":true,"showFps":true,"enviromentOptions":[{"key":"DRI_PRIME","value":"0"}]"#,
    );
    let import = source.inspect_native("hades").unwrap();
    assert_eq!(import.env["DRI_PRIME"], "0");
    assert_eq!(import.env["__NV_PRIME_RENDER_OFFLOAD"], "1");
    assert_eq!(import.env["__GLX_VENDOR_LIBRARY_NAME"], "nvidia");
    // DXVK_HUD is Wine's, so showFps means nothing to a native game.
    assert!(!import.env.contains_key("DXVK_HUD"));
    assert!(import.settings_review.carried.contains(&review_item(
        "nvidiaPrime",
        "DRI_PRIME=1 and NVIDIA offload variables"
    )));
}

#[test]
fn a_native_import_keeps_what_wineo_manages_only_for_wine() {
    let (_root, source) = native_game(
        r#""enviromentOptions":[{"key":"WINEPREFIX","value":"/mine"},{"key":"PROTONPATH","value":"/proton"}]"#,
    );
    let import = source.inspect_native("hades").unwrap();
    assert_eq!(import.env["WINEPREFIX"], "/mine");
    assert_eq!(import.env["PROTONPATH"], "/proton");
    assert!(
        !import
            .issues
            .iter()
            .any(|issue| issue.kind == HeroicIssueKind::Conflict)
    );
}

#[test]
fn fingerprint_detects_source_changes() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("game.exe").display()
    );
    write_fixture(&source, &global, "", &library);
    let first = source.fingerprint("hades").unwrap();
    assert!(source.unchanged("hades", &first).unwrap());
    fs::write(source.library_path(), library.replace("Hades", "Changed")).unwrap();
    assert!(!source.unchanged("hades", &first).unwrap());
}

/// A Windows pair and a native game in one library, with the library text.
fn three_games(root: &Path, source: &HeroicSource) -> String {
    let prefix = wine_prefix(root);
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let exe = prefix.join("game.exe").display().to_string();
    let library = format!(
        r#"{{"games":[
                {{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{exe}"}}}},
                {{"app_name":"other","title":"Other","runner":"sideload","install":{{"platform":"Windows","executable":"{exe}"}}}},
                {{"app_name":"dusk","title":"Dusk","runner":"sideload","install":{{"platform":"linux","executable":"/usr/bin/dusk"}}}}
            ]}}"#
    );
    write_fixture(source, &global, "", &library);
    library
}

fn read_inputs(source: &HeroicSource) -> (Library, Global) {
    (
        source.read_library().unwrap(),
        source.read_global().unwrap(),
    )
}

#[test]
fn an_import_from_files_read_earlier_is_fingerprinted_by_what_was_read() {
    let (root, source) = fixture();
    let library = three_games(root.path(), &source);
    let (read_library, read_global) = read_inputs(&source);
    let inputs = Inputs {
        library: &read_library,
        global: &read_global,
    };
    // Nothing changed, so it fingerprints like a fresh read.
    assert_eq!(
        source.inspect_in(inputs, "other").unwrap().fingerprint,
        source.fingerprint("other").unwrap()
    );

    // Heroic renames one game and drops another after the read. Both are
    // still planned from what was read, and neither passes the check
    // that apply makes.
    let changed = library
        .replace("Other", "Renamed")
        .replace("\"dusk\"", "\"gone\"");
    fs::write(source.library_path(), changed).unwrap();
    let other = source.inspect_in(inputs, "other").unwrap();
    assert_eq!(other.title, "Other");
    assert!(!source.unchanged("other", &other.fingerprint).unwrap());
    let dusk = source.inspect_native_in(inputs, "dusk").unwrap();
    assert_eq!(dusk.title, "Dusk");
    assert!(!source.unchanged("dusk", &dusk.fingerprint).unwrap());
}

#[test]
fn a_change_to_the_global_settings_after_they_were_read_fails_the_check() {
    let (root, source) = fixture();
    three_games(root.path(), &source);
    let (read_library, read_global) = read_inputs(&source);
    let inputs = Inputs {
        library: &read_library,
        global: &read_global,
    };
    let config = fs::read_to_string(source.config_path()).unwrap();
    fs::write(
        source.config_path(),
        config.replace(
            r#""defaultSettings":{"#,
            r#""defaultSettings":{"showFps":true,"#,
        ),
    )
    .unwrap();
    for fingerprint in [
        source.inspect_in(inputs, "other").unwrap().fingerprint,
        source
            .inspect_native_in(inputs, "dusk")
            .unwrap()
            .fingerprint,
    ] {
        assert!(!source.unchanged("other", &fingerprint).unwrap());
    }
}

/// Has Heroic change the game's launcher args after its config was read
/// and before it is parsed.
fn change_args_mid_inspection(source: &HeroicSource) {
    let path = source.game_config_path("hades");
    AFTER_GAME_READ.set(Some(Box::new(move || {
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("--old", "--new")).unwrap();
    })));
}

#[test]
fn a_game_config_changed_during_inspection_fails_the_check() {
    for native in [false, true] {
        let (root, source) = fixture();
        game_fixture(&source, root.path(), r#"{"launcherArgs":"--old"}"#, native);
        change_args_mid_inspection(&source);
        let (launcher_args, fingerprint) = if native {
            let import = source.inspect_native("hades").unwrap();
            (import.launcher_args, import.fingerprint)
        } else {
            let import = source.inspect("hades").unwrap();
            (import.launcher_args, import.fingerprint)
        };
        // The settings are what was read, so the fingerprint must be too.
        assert_eq!(launcher_args, "--old");
        assert!(
            !source.unchanged("hades", &fingerprint).unwrap(),
            "native: {native}"
        );
    }
}

#[test]
fn an_unchanged_import_fingerprints_like_a_fresh_read() {
    for native in [false, true] {
        let (root, source) = fixture();
        game_fixture(&source, root.path(), r#"{"launcherArgs":"--old"}"#, native);
        let fingerprint = if native {
            source.inspect_native("hades").unwrap().fingerprint
        } else {
            source.inspect("hades").unwrap().fingerprint
        };
        assert_eq!(fingerprint, source.fingerprint("hades").unwrap());
    }
}

#[test]
fn an_unchanged_gog_inspection_fingerprints_like_gog_fingerprint() {
    use crate::gog::folder::Platform;

    for native in [false, true] {
        let (root, source) = fixture();
        game_fixture(&source, root.path(), r#"{"launcherArgs":"--old"}"#, native);
        fs::create_dir_all(source.config_dir().join("gog_store")).unwrap();
        fs::write(source.gog_installed_path(), r#"{"installed":[]}"#).unwrap();
        let info = root.path().join("goggame-1.info");
        fs::write(&info, "{}").unwrap();
        let game = HeroicGogGame {
            id: "hades".to_owned(),
            title: "Hades".to_owned(),
            platform: if native {
                Platform::Linux
            } else {
                Platform::Windows
            },
            folder: root.path().join("game"),
            version: String::new(),
            language: String::new(),
            build_id: None,
            dlc: Vec::new(),
        };
        let exe = root.path().join("prefix/drive_c/game.exe");
        let fingerprint = if native {
            source
                .inspect_gog_native(&game, &exe, &info)
                .unwrap()
                .fingerprint
        } else {
            source.inspect_gog(&game, &exe, &info).unwrap().fingerprint
        };
        assert_eq!(
            fingerprint,
            source.gog_fingerprint(&game, &info).unwrap(),
            "native: {native}"
        );
    }
}

#[test]
fn discovery_finds_native_and_flatpak_and_deduplicates_aliases() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let native = home.path().join(".config/heroic");
    fs::create_dir_all(native.join("sideload_apps")).unwrap();
    let flatpak = home
        .path()
        .join(".var/app/com.heroicgameslauncher.hgl/config/heroic");
    fs::create_dir_all(flatpak.join("sideload_apps")).unwrap();
    let sources = HeroicSource::discover(home.path(), None, None).unwrap();
    assert_eq!(sources.len(), 2);
    assert!(
        sources
            .iter()
            .any(|source| source.origin() == HeroicOrigin::Native)
    );
    assert!(
        sources
            .iter()
            .any(|source| source.origin() == HeroicOrigin::Flatpak)
    );

    let alias_root = home
        .path()
        .join(".var/app/com.heroicgameslauncher.hgl/.config");
    fs::create_dir_all(&alias_root).unwrap();
    symlink(&native, alias_root.join("heroic")).unwrap();
    let sources = HeroicSource::discover(home.path(), None, None).unwrap();
    assert_eq!(sources.len(), 2);
}

#[test]
fn unsupported_schema_isolated_to_game_and_global_version_is_strict() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"version":"v0","defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("game.exe").display()
    );
    write_fixture(&source, &global, r#"{"version":"v9","hades":{}}"#, &library);
    let listing = source.list_candidates().unwrap();
    assert!(
        listing.candidates[0]
            .issues
            .iter()
            .any(|issue| issue.kind == HeroicIssueKind::Invalid)
    );

    fs::write(source.config_path(), global.replace("\"v0\"", "\"v9\"")).unwrap();
    assert!(source.list_candidates().is_err());
}

#[test]
fn toggle_translation_handles_system_wine_target_and_unknown_overrides() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"enableEsync":true,"enableFsync":true,"enviromentOptions":[{{"key":"DXVK_HUD","value":"full"}}],"analyticsOptIn":true,"steamGridDbApiKey":"secret"}}}}"#,
        prefix.display()
    );
    let game = format!(
        r#"{{"version":"v0.1","hades":{{"targetExe":"{}","launcherArgs":"\"\" --name 'two words'","showFps":true,"unknownLaunchMode":true}}}}"#,
        prefix.join("drive_c/Custom.exe").display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("game.exe").display()
    );
    write_fixture(&source, &global, &game, &library);
    let import = source.inspect("hades").unwrap();
    assert_eq!(import.exe, prefix.join("drive_c/Custom.exe"));
    assert_eq!(import.args, ["", "--name", "two words"]);
    assert_eq!(import.env.get("DXVK_HUD"), Some(&"fps".to_owned()));
    assert_eq!(import.env.get("WINEESYNC"), Some(&"1".to_owned()));
    assert_eq!(import.env.get("WINEFSYNC"), Some(&"1".to_owned()));
    assert!(
        !import
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("steamGridDbApiKey"))
    );
    assert!(
        import
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("unknownLaunchMode"))
    );
    assert!(
        import
            .settings_review
            .carried
            .iter()
            .all(|item| !item.detail.contains("secret"))
    );
}

#[test]
fn multiple_games_can_share_a_prefix() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"one","title":"One","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"two","title":"Two","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("one.exe").display(),
        prefix.join("two.exe").display()
    );
    fs::write(source.config_path(), global).unwrap();
    fs::write(source.library_path(), library).unwrap();
    assert_eq!(source.list_candidates().unwrap().candidates.len(), 2);
    assert_eq!(
        source.inspect("one").unwrap().prefix,
        source.inspect("two").unwrap().prefix
    );
}

#[test]
fn shared_prefix_candidates_resolve_aliases_and_proton_pfx() {
    use std::os::unix::fs::symlink;

    let (root, source) = fixture();
    let compatdata = root.path().join("compatdata");
    fs::create_dir_all(compatdata.join("pfx/drive_c")).unwrap();
    fs::write(
        compatdata.join("pfx/system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    symlink(&compatdata, root.path().join("alias")).unwrap();
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"proton","name":"GE-Proton","bin":"/runners/GE-Proton/proton"}}}}}}"#,
        compatdata.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"one","title":"One","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"two","title":"Two","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"other","title":"Other","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        compatdata.join("pfx/drive_c/one.exe").display(),
        compatdata.join("pfx/drive_c/two.exe").display(),
        compatdata.join("pfx/drive_c/other.exe").display(),
    );
    fs::write(source.config_path(), global).unwrap();
    fs::write(source.library_path(), library).unwrap();
    fs::write(
        source.game_config_path("two"),
        format!(
            r#"{{"version":"v0.1","two":{{"winePrefix":"{}"}}}}"#,
            root.path().join("alias/pfx").display()
        ),
    )
    .unwrap();
    let other = wine_prefix(root.path().join("separate").as_path());
    fs::write(
        source.game_config_path("other"),
        format!(
            r#"{{"version":"v0.1","other":{{"winePrefix":"{}"}}}}"#,
            other.display()
        ),
    )
    .unwrap();
    assert_eq!(
        source.inspect("two").unwrap().prefix,
        root.path().join("alias")
    );
    assert_eq!(
        source.shared_prefix_candidates("one").unwrap(),
        vec![HeroicSharedPrefix {
            id: "two".to_owned(),
            title: "Two".to_owned(),
        }]
    );
}

#[test]
fn disable_umu_reports_only_per_game_true_for_proton() {
    let (root, source) = fixture();
    let prefix = root.path().join("compatdata");
    fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
    fs::write(prefix.join("pfx/system.reg"), "#arch=win64\n").unwrap();
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"proton","name":"GE-Proton","bin":"/runners/GE-Proton/proton"}},"disableUMU":true}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("pfx/drive_c/game.exe").display()
    );
    write_fixture(&source, &global, "", &library);
    assert!(
        !source
            .inspect("hades")
            .unwrap()
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("disableUMU"))
    );
    fs::write(
        source.game_config_path("hades"),
        r#"{"version":"v0.1","hades":{"disableUMU":false}}"#,
    )
    .unwrap();
    assert!(
        !source
            .inspect("hades")
            .unwrap()
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("disableUMU"))
    );
    fs::write(
        source.game_config_path("hades"),
        r#"{"version":"v0.1","hades":{"disableUMU":true}}"#,
    )
    .unwrap();
    let imported = source.inspect("hades").unwrap();
    assert!(
        imported
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("disableUMU")
                && issue.kind == HeroicIssueKind::Unsupported
                && issue.message.contains("wineo uses umu"))
    );
    assert!(
        imported
            .settings_review
            .unsupported
            .iter()
            .any(|item| item.field == "disableUMU")
    );
}

#[test]
fn wine_nvapi_warns_about_dll_requirement_without_claiming_absence() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}},"autoInstallDxvkNvapi":true,"disableUMU":true}}}}"#,
        prefix.display()
    );
    let library = format!(
        r#"{{"games":[{{"app_name":"hades","title":"Hades","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
        prefix.join("drive_c/game.exe").display()
    );
    write_fixture(
        &source,
        &global,
        r#"{"version":"v0.1","hades":{"disableUMU":true}}"#,
        &library,
    );
    let imported = source.inspect("hades").unwrap();
    assert_eq!(imported.env.get("DXVK_ENABLE_NVAPI"), Some(&"1".to_owned()));
    assert!(imported.issues.iter().any(|issue| issue.field.as_deref()
        == Some("autoInstallDxvkNvapi")
        && issue.message.contains("requires dxvk-nvapi DLLs")
        && !issue.message.contains("missing")));
    assert!(
        !imported
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("disableUMU"))
    );
}

#[test]
fn playtime_reads_total_minutes_and_last_played() {
    let (_root, source) = fixture();
    fs::create_dir_all(source.config_dir().join("store")).unwrap();
    fs::write(
            source.config_dir().join("store/timestamp.json"),
            r#"{"hades":{"firstPlayed":"2026-06-01T10:00:00.000Z","lastPlayed":"2026-09-20T19:02:11.000Z","totalPlayed":5720}}"#,
        )
        .unwrap();
    let playtime = source.playtime("hades").unwrap().unwrap();
    assert_eq!(playtime.seconds, 5720 * 60);
    assert_eq!(
        playtime.last_played,
        Some("2026-09-20T19:02:11Z".parse().unwrap())
    );
}

#[test]
fn playtime_is_none_without_a_played_entry_or_file() {
    let (_root, source) = fixture();
    assert_eq!(source.playtime("hades").unwrap(), None);

    fs::create_dir_all(source.config_dir().join("store")).unwrap();
    fs::write(
            source.config_dir().join("store/timestamp.json"),
            r#"{"hades":{"firstPlayed":"","lastPlayed":"","totalPlayed":0},"other":{"firstPlayed":"2026-01-01T00:00:00.000Z","lastPlayed":"2026-01-02T00:00:00.000Z","totalPlayed":90}}"#,
        )
        .unwrap();
    assert_eq!(source.playtime("hades").unwrap(), None);
    assert_eq!(source.playtime("missing").unwrap(), None);
    let other = source.playtime("other").unwrap().unwrap();
    assert_eq!(other.seconds, 90 * 60);
    assert_eq!(
        other.last_played,
        Some("2026-01-02T00:00:00Z".parse().unwrap())
    );
}

const POSTER_URL: &str = "https://cdn.example.com/art/Poster.JPG";
const POSTER_DIGEST: &str = "44f8d500fbcdedb36b8b6ece0c50218a82c365af8d8d4b9456ebab732d1e94cb";

#[test]
fn cached_images_are_named_by_the_lowercase_sha256_of_the_url() {
    let home = tempfile::tempdir().unwrap();
    let native = home.path().join(".config/heroic");
    fs::create_dir_all(native.join("sideload_apps")).unwrap();
    let flatpak = home
        .path()
        .join(".var/app/com.heroicgameslauncher.hgl/config/heroic");
    fs::create_dir_all(flatpak.join("sideload_apps")).unwrap();
    let sources = HeroicSource::discover(home.path(), None, None).unwrap();
    assert_eq!(sources.len(), 2);
    for source in sources {
        let expected = match source.origin() {
            HeroicOrigin::Native => native.join("images-cache").join(POSTER_DIGEST),
            _ => flatpak.join("images-cache").join(POSTER_DIGEST),
        };
        assert_eq!(source.cached_image(POSTER_URL), expected);
    }
}

#[test]
fn a_listing_finds_cached_posters_and_skips_missing_ones() {
    let (root, source) = fixture();
    let prefix = wine_prefix(root.path());
    let global = format!(
        r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
        prefix.display()
    );
    let exe = prefix.join("game.exe").display().to_string();
    let library = format!(
        r#"{{"games":[
                {{"app_name":"cached","title":"Cached","runner":"sideload","art_square":"{POSTER_URL}","install":{{"platform":"Windows","executable":"{exe}"}}}},
                {{"app_name":"uncached","title":"Uncached","runner":"sideload","art_square":"https://cdn.example.com/other.jpg","install":{{"platform":"Windows","executable":"{exe}"}}}},
                {{"app_name":"bare","title":"Bare","runner":"sideload","art_cover":"{POSTER_URL}","install":{{"platform":"Windows","executable":"{exe}"}}}}
            ]}}"#
    );
    write_fixture(&source, &global, "", &library);
    let cache = source.config_dir().join("images-cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join(POSTER_DIGEST), b"\xff\xd8\xff poster").unwrap();
    let listing = source.list_candidates().unwrap();
    let poster = |id: &str| {
        listing
            .candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .unwrap()
            .poster
            .clone()
    };
    assert_eq!(poster("cached"), Some(cache.join(POSTER_DIGEST)));
    assert_eq!(poster("uncached"), None);
    // art_cover is Heroic's landscape art, not the poster.
    assert_eq!(poster("bare"), None);
}
