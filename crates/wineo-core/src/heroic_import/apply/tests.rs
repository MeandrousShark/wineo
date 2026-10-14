use crate::test_script::write_script;
use std::fs;

use tempfile::tempdir;

use super::*;
use crate::config::Defaults;
use crate::heroic_import::plan::tests::{fake_wine_runner, paths};
use crate::heroic_import::{Options, plan_game};

/// A Heroic config with one sideloaded game, Alpha, whose prefix is
/// `root/heroic-prefix`, and a fake wine runner named `fake`.
fn library(root: &Path) -> (Paths, GlobalConfig, HeroicSource) {
    let prefix = root.join("heroic-prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    let exe = prefix.join("drive_c/Games/Game.exe");
    fs::write(&exe, "game executable").unwrap();
    let config = root.join("heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    fs::create_dir_all(config.join("store")).unwrap();
    fs::write(
            config.join("config.json"),
            format!(
                r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
                prefix.display()
            ),
        )
        .unwrap();
    fs::write(config.join("GamesConfig/a.json"), r#"{"a":{}}"#).unwrap();
    fs::write(
            config.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
    let runner_root = root.join("runners");
    fake_wine_runner(&runner_root, "fake");
    let global = GlobalConfig {
        defaults: Defaults {
            runner_root: Some(runner_root.display().to_string()),
            prefix_root: Some(root.join("prefixes").display().to_string()),
            ..Defaults::default()
        },
        ..GlobalConfig::default()
    };
    let source = HeroicSource::from_config_dir_with_home(config, root.to_owned());
    (paths(root), global, source)
}

fn plan(paths: &Paths, global: &GlobalConfig, source: &HeroicSource) -> GamePlan {
    plan_game(paths, global, source, "a", "fake", Options::default()).unwrap()
}

#[test]
fn apply_registers_the_plan_and_carries_playtime() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    fs::write(
        source.config_dir().join("store/timestamp.json"),
        r#"{"a":{"totalPlayed":90,"lastPlayed":"2024-01-02T03:04:05Z"}}"#,
    )
    .unwrap();
    let mut plan = plan(&paths, &global, &source);
    plan.set_desktop_enabled(true);
    let mut events = Vec::new();
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |event| {
        events.push(event)
    })
    .unwrap();
    assert_eq!(registered.environment.name, "alpha");
    assert_eq!(
        registered.environment.config.heroic_id.as_deref(),
        Some("a")
    );
    assert_eq!(registered.playtime, Playtime::Imported);
    assert_eq!(registered.runner().unwrap().name(), "fake");
    assert!(registered.environment.config.hooks.is_none());
    assert!(
        !fs::read_to_string(paths.environment_config("alpha"))
            .unwrap()
            .contains("[hooks]")
    );
    let copy = root.path().join("prefixes/alpha");
    assert_eq!(registered.destination(), Some(copy.as_path()));
    assert!(copy.join("system.reg").is_file());
    // Source checks report before the copy starts.
    assert!(matches!(
        events
            .iter()
            .find(|event| !matches!(event, ImportProgress::Checking { .. })),
        Some(ImportProgress::Started { .. })
    ));
    assert!(matches!(
        events.last(),
        Some(ImportProgress::Finished { .. })
    ));
    let log = fs::read_to_string(paths.stats_log("alpha")).unwrap();
    assert!(log.contains("5400"), "{log}");

    // The desktop entry is written by the second call.
    let changes = registered.finish(&paths, &global).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, Change::Wrote { .. })),
        "{changes:?}"
    );
}

#[test]
fn apply_carries_heroic_scripts_into_validated_hooks() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let before = "/Heroic scripts/before.sh";
    let after = "/Heroic scripts/after.sh";
    fs::write(
        source.config_dir().join("GamesConfig/a.json"),
        serde_json::json!({
            "a": {
                "beforeLaunchScriptPath": before,
                "afterLaunchScriptPath": after,
            }
        })
        .to_string(),
    )
    .unwrap();
    let plan = plan(&paths, &global, &source);
    assert!(
        plan.heroic()
            .settings_review
            .carried
            .contains(&crate::heroic::ReviewItem {
                field: "before hook".to_owned(),
                detail: before.to_owned(),
            })
    );
    assert!(
        plan.heroic()
            .settings_review
            .carried
            .contains(&crate::heroic::ReviewItem {
                field: "after hook".to_owned(),
                detail: after.to_owned(),
            })
    );

    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    let hooks = registered
        .environment
        .config
        .hooks
        .as_ref()
        .and_then(crate::config::HooksSetting::on)
        .unwrap();
    assert_eq!(hooks.before.as_deref(), Some("'/Heroic scripts/before.sh'"));
    assert_eq!(hooks.after.as_deref(), Some("'/Heroic scripts/after.sh'"));
}

#[test]
fn apply_reloads_and_runs_a_safely_quoted_heroic_script_path() {
    use std::process::Command;

    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let script = root.path().join("script ' $ ;\nname.sh");
    let marker = root.path().join("script-ran");
    write_script(&script, "#!/bin/sh\nprintf ran > \"$MARKER\"\n");
    fs::write(
        source.config_dir().join("GamesConfig/a.json"),
        serde_json::json!({
            "a": {
                "beforeLaunchScriptPath": script.to_str().unwrap(),
            }
        })
        .to_string(),
    )
    .unwrap();

    let plan = plan(&paths, &global, &source);
    apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    let reloaded = crate::environment::Environment::load(&paths, "alpha").unwrap();
    let hook = reloaded
        .config
        .hooks
        .as_ref()
        .and_then(crate::config::HooksSetting::on)
        .and_then(|hooks| hooks.before.as_deref())
        .unwrap();
    let status = Command::new("sh")
        .arg("-c")
        .arg(hook)
        .env("MARKER", marker)
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(root.path().join("script-ran")).unwrap(),
        "ran"
    );
}

#[test]
fn apply_can_adopt_and_leaves_the_prefix_where_it_is() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let plan = plan_game(
        &paths,
        &global,
        &source,
        "a",
        "fake",
        Options {
            name: Some("mine".to_owned()),
            adopt: true,
            ..Options::default()
        },
    )
    .unwrap();
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(registered.playtime, Playtime::None);
    assert_eq!(
        registered.destination(),
        Some(root.path().join("heroic-prefix").as_path())
    );
    assert!(!root.path().join("prefixes").exists());
    assert!(registered.finish(&paths, &global).unwrap().is_empty());
}

#[test]
fn apply_refuses_when_heroic_changed_since_the_game_was_inspected() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let plan = plan(&paths, &global, &source);
    fs::write(
        source.config_dir().join("GamesConfig/a.json"),
        r#"{"a":{"useGameMode":true}}"#,
    )
    .unwrap();
    let error = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ApplyError>(),
        Some(ApplyError::SourceChanged)
    ));
    assert!(!paths.environment_config("alpha").exists());
}

#[test]
fn a_canceled_apply_leaves_nothing_behind() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let plan = plan(&paths, &global, &source);
    let token = Token::new();
    token.cancel();
    let error = apply(&paths, &global, &source, &plan, &token, |_| {}).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<crate::Error>(),
            Some(crate::Error::Canceled(_))
        ) || error.downcast_ref::<import::Cancelled>().is_some(),
        "{error:#}"
    );
    assert!(!paths.environment_config("alpha").exists());
    assert!(!root.path().join("prefixes/alpha").exists());
}

#[test]
fn an_unreadable_playtime_is_reported_not_fatal() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    fs::write(source.config_dir().join("store/timestamp.json"), "not json").unwrap();
    let plan = plan(&paths, &global, &source);
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    assert!(matches!(registered.playtime, Playtime::Unreadable(_)));
    assert!(paths.environment_config("alpha").exists());
}

#[test]
fn import_log_appends_a_timestamped_section_each_time() {
    let root = tempdir().unwrap();
    let body = "Heroic game: Game (game)\n  desktop entry: Game\n";
    let paths = paths(root.path());
    let log_path = write_import_log(&paths, "game", body).unwrap();
    assert_eq!(log_path, paths.import_log("game"));
    let contents = fs::read_to_string(&log_path).unwrap();
    assert!(contents.contains("=== "), "{contents}");
    assert!(contents.contains("Heroic game: Game (game)"), "{contents}");

    // A second write appends another timestamped section.
    write_import_log(&paths, "game", body).unwrap();
    let contents = fs::read_to_string(&log_path).unwrap();
    assert_eq!(contents.matches("=== ").count(), 2, "{contents}");
}

/// Every file under `dir` with its bytes, to show nothing there changed.
fn tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(tree(&path));
        } else {
            files.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
    files
}

#[test]
fn a_heroic_poster_is_stored_as_manual_grid_art_that_fetching_keeps() {
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0 poster";
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let cache = source.config_dir().join("images-cache");
    fs::create_dir_all(&cache).unwrap();
    let poster = cache.join("0123abcd");
    fs::write(&poster, JPEG).unwrap();
    let before = tree(source.config_dir());
    let plan = plan(&paths, &global, &source);
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    let name = registered.environment.name.as_str();

    let stored = store_poster(&paths, name, "a", &poster).unwrap().unwrap();
    assert_eq!(stored.format, art::Format::Jpeg);
    assert_eq!(
        fs::read(paths.art_dir(name).join("grid.jpg")).unwrap(),
        JPEG
    );
    let record = |paths: &Paths| {
        art::ArtConfig::load(&paths.art_dir(name)).unwrap().kinds[&Kind::Grid]
            .source
            .clone()
    };
    assert_eq!(record(&paths), art::Source::Manual("heroic-a.jpg".into()));

    let fetched = art::Downloaded {
        asset: 42,
        bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
        format: art::Format::Png,
    };
    crate::art_fetch::publish(
        &paths,
        name,
        None,
        Kind::Grid,
        &fetched,
        crate::art_fetch::Mode::Fill,
    )
    .unwrap();
    assert_eq!(record(&paths), art::Source::Manual("heroic-a.jpg".into()));
    assert!(!paths.art_dir(name).join("grid.png").exists());
    assert_eq!(tree(source.config_dir()), before);
}

#[test]
fn a_poster_is_judged_by_its_signature_alone() {
    // Callers decode posters first; this pins down why they must.
    const CORRUPT: &[u8] = b"\xff\xd8\xff\xe0 corrupt body";
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let plan = plan(&paths, &global, &source);
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    let name = registered.environment.name.as_str();
    let poster = root.path().join("corrupt");
    fs::write(&poster, CORRUPT).unwrap();
    let stored = store_poster(&paths, name, "a", &poster).unwrap().unwrap();
    assert_eq!(stored.format, art::Format::Jpeg);
    assert_eq!(fs::read(stored.path).unwrap(), CORRUPT);
}

#[test]
fn a_missing_or_unusable_poster_is_skipped() {
    let root = tempdir().unwrap();
    let (paths, global, source) = library(root.path());
    let plan = plan(&paths, &global, &source);
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    let name = registered.environment.name.as_str();
    let missing = root.path().join("gone");
    assert_eq!(store_poster(&paths, name, "a", &missing).unwrap(), None);
    // Icons can be ICO, but Grid art can't.
    let ico = root.path().join("icon");
    let mut bytes = vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0];
    bytes.extend(4u32.to_le_bytes());
    bytes.extend(22u32.to_le_bytes());
    bytes.extend([0; 4]);
    assert_eq!(art::detect(&bytes), Some(art::Format::Ico));
    fs::write(&ico, bytes).unwrap();
    assert_eq!(store_poster(&paths, name, "a", &ico).unwrap(), None);
    let text = root.path().join("text");
    fs::write(&text, b"not an image").unwrap();
    assert_eq!(store_poster(&paths, name, "a", &text).unwrap(), None);
    assert!(!paths.art_dir(name).join(art::ART_FILE).exists());
}
