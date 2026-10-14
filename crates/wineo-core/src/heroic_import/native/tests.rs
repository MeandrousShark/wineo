use crate::test_script::write_script;
use std::fs;

use tempfile::tempdir;

use super::*;
use crate::heroic::HeroicIssue;
use crate::heroic_import::plan::tests::paths;
use crate::heroic_import::{ApplyError, Playtime, apply_native};
use crate::test_snapshot::{assert_unchanged, snapshot_tree};

/// A Heroic library holding one native sideload, Dusklight, whose script
/// is in the home directory `root`. `settings` is the game's own config.
fn dusklight(root: &Path, settings: &str) -> HeroicSource {
    let heroic = root.join("heroic");
    fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
    fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
    fs::create_dir_all(heroic.join("store")).unwrap();
    let exe = root.join("games/dusklight");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(&exe, "#!/bin/sh\n");
    fs::write(
            heroic.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
    fs::write(
        heroic.join("GamesConfig/dusk.json"),
        format!(r#"{{"dusk":{settings}}}"#),
    )
    .unwrap();
    HeroicSource::from_config_dir_with_home(heroic, root.to_owned())
}

#[test]
fn native_planning_and_exe_check_leave_the_roots_unchanged() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), "{}");
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let before = snapshot_tree(root.path());

    let plan = plan_native(&source, "dusk", None).unwrap();
    plan.check_exe(&paths, &global).unwrap();

    assert_unchanged(&before, &snapshot_tree(root.path()));
}

#[test]
fn native_launch_edits_preserve_the_heroic_source_and_resolve_arguments() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), r#"{"launcherArgs":"\"unclosed"}"#);
    let mut plan = plan_native(&source, "dusk", None).unwrap();
    assert!(plan.reject_invalid().is_err());
    let original = plan.heroic().clone();
    let exe = root.path().join("another-game");
    let cwd = root.path().join("working");
    assert!(plan.set_executable(PathBuf::new()).is_err());
    plan.set_executable(exe.clone()).unwrap();
    plan.set_working_directory(Some(cwd.clone()));
    plan.set_launcher_args("--name 'two words'").unwrap();
    assert!(plan.reject_invalid().is_ok());
    let settings = plan.settings();
    assert_eq!(settings.targets["game"].exe, exe.to_string_lossy());
    assert_eq!(settings.targets["game"].cwd.as_deref(), cwd.to_str());
    assert_eq!(settings.targets["game"].args, ["--name", "two words"]);
    assert_eq!(plan.heroic().exe, original.exe);
    assert_eq!(plan.heroic().args, original.args);
    assert_eq!(plan.heroic().issues, original.issues);
    assert!(plan.set_launcher_args("'unclosed").is_err());
    assert_eq!(
        plan.settings().targets["game"].args,
        ["--name", "two words"]
    );
}

#[test]
fn native_setting_edits_keep_the_source_and_unrelated_issues() {
    let root = tempdir().unwrap();
    let source = dusklight(
        root.path(),
        r#"{"useGameMode":true,"enviromentOptions":[{"key":"OLD","value":"old"}]}"#,
    );
    let mut plan = plan_native(&source, "dusk", None).unwrap();
    let original = plan.heroic().clone();
    plan.set_environment(["NEW=value"]).unwrap();
    plan.set_gamemode(false);
    plan.set_mangohud(true);
    plan.set_gamescope(Some(GamescopeSetting::On(Default::default())));
    let settings = plan.settings();
    assert_eq!(settings.env["NEW"], "value");
    assert_eq!(settings.gamemode, Some(false));
    assert_eq!(settings.mangohud, Some(true));
    assert!(matches!(settings.gamescope, Some(GamescopeSetting::On(_))));
    assert_eq!(plan.heroic().env, original.env);
    assert_eq!(plan.heroic().issues, original.issues);
    assert!(plan.set_environment(["INVALID"]).is_err());
    assert_eq!(plan.settings().env["NEW"], "value");
}

#[test]
fn a_native_plan_is_named_from_its_title_and_carries_its_settings() {
    let root = tempdir().unwrap();
    let source = dusklight(
        root.path(),
        r#"{"useGameMode":true,"launcherArgs":"--fast \"a b\"","enviromentOptions":[{"key":"FOO","value":"bar"}]}"#,
    );
    let mut plan = plan_native(&source, "dusk", None).unwrap();
    assert_eq!(plan.name(), "dusklight");
    assert_eq!(plan.desktop().name, "Dusklight");
    assert!(!plan.desktop().enabled);
    plan.set_desktop_enabled(true);
    plan.set_desktop_name("Dusk").unwrap();
    let settings = plan.settings();
    assert_eq!(settings.heroic_id.as_deref(), Some("dusk"));
    assert_eq!(settings.gamemode, Some(true));
    assert_eq!(settings.env["FOO"], "bar");
    let target = &settings.targets["game"];
    assert_eq!(target.args, ["--fast", "a b"]);
    assert!(target.default);
    assert!(target.desktop);
    assert_eq!(target.name.as_deref(), Some("Dusk"));
    assert!(settings.dxvk.is_none() && settings.wayland.is_none());

    plan.set_desktop_enabled(false);
    assert_eq!(plan.settings().targets["game"].name, None);
    plan.set_gamemode(false);
    assert_eq!(plan.settings().gamemode, Some(false));
    // Heroic's own report keeps what it said.
    assert_eq!(plan.heroic().gamemode, Some(true));
    assert_eq!(plan.resolved().gamemode, Some(false));
}

#[test]
fn a_name_is_validated_and_can_be_chosen_up_front() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), "{}");
    let mut plan = plan_native(&source, "dusk", Some("mine".to_owned())).unwrap();
    assert_eq!(plan.name(), "mine");
    assert!(matches!(
        plan.set_name("not ok"),
        Err(PlanError::InvalidName(_))
    ));
    assert_eq!(plan.name(), "mine");
    plan.set_name("renamed").unwrap();
    assert_eq!(plan.name(), "renamed");
    assert!(matches!(
        plan.set_desktop_name(""),
        Err(PlanError::EmptyDesktopName)
    ));
}

#[test]
fn a_game_with_unparseable_settings_is_refused() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), r#"{"launcherArgs":"-a \"open"}"#);
    let plan = plan_native(&source, "dusk", None).unwrap();
    let error = plan.reject_invalid().unwrap_err();
    assert!(
        error.to_string().contains("invalid Heroic quoting"),
        "{error}"
    );
    let source = dusklight(root.path(), "{}");
    assert!(
        plan_native(&source, "dusk", None)
            .unwrap()
            .reject_invalid()
            .is_ok()
    );
}

#[test]
fn an_exe_that_cannot_run_is_reported_the_way_a_launch_would() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), "{}");
    let plan = plan_native(&source, "dusk", None).unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    assert!(plan.check_exe(&paths, &global).is_ok());
    fs::remove_file(root.path().join("games/dusklight")).unwrap();
    let error = plan.check_exe(&paths, &global).unwrap_err();
    assert!(
        error.to_string().starts_with("executable not found: "),
        "{error}"
    );
}

fn issue(kind: HeroicIssueKind, field: &str) -> HeroicIssue {
    HeroicIssue {
        kind,
        field: Some(field.to_owned()),
        message: format!("{field} issue"),
    }
}

#[test]
fn warnings_count_omitted_settings_and_flag_what_needs_attention() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), "{}");
    let mut imported = source.inspect_native("dusk").unwrap();
    let warnings = |imported: &HeroicNativeImport| {
        NativePlan::new(imported.clone(), "x".to_owned()).warnings()
    };
    assert!(warnings(&imported).is_empty());
    imported.issues = vec![
        issue(HeroicIssueKind::Unsupported, "wrapperOptions"),
        issue(HeroicIssueKind::Unsupported, "enableWoW64"),
    ];
    assert_eq!(warnings(&imported), vec![Warning::Omitted(2)]);
    imported.issues = vec![
        issue(HeroicIssueKind::Conflict, "useGameMode"),
        issue(HeroicIssueKind::Unsupported, "wrapperOptions"),
    ];
    assert_eq!(
        warnings(&imported),
        vec![
            Warning::Omitted(1),
            Warning::NeedsAttention(vec![issue(HeroicIssueKind::Conflict, "useGameMode")])
        ]
    );
}

#[test]
fn apply_native_registers_the_environment_and_carries_playtime() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), r#"{"useGameMode":true}"#);
    fs::write(
        source.config_dir().join("store/timestamp.json"),
        r#"{"dusk":{"totalPlayed":10}}"#,
    )
    .unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let mut plan = plan_native(&source, "dusk", None).unwrap();
    plan.set_desktop_enabled(true);
    let registered = apply_native(&paths, &source, &plan).unwrap();
    assert_eq!(registered.environment.name, "dusklight");
    assert_eq!(
        registered.environment.config.heroic_id.as_deref(),
        Some("dusk")
    );
    assert_eq!(registered.playtime, Playtime::Imported);
    assert!(registered.runner().is_none() && registered.destination().is_none());
    assert!(registered.environment.config.hooks.is_none());
    assert!(
        !fs::read_to_string(paths.environment_config("dusklight"))
            .unwrap()
            .contains("[hooks]")
    );
    let log = fs::read_to_string(paths.stats_log("dusklight")).unwrap();
    assert!(log.contains("600"), "{log}");
    let changes = registered.finish(&paths, &global).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, crate::desktop::Change::Wrote { .. })),
        "{changes:?}"
    );
    // The same name can't be registered twice.
    assert!(apply_native(&paths, &source, &plan).is_err());
}

#[test]
fn apply_native_carries_heroic_scripts_into_validated_hooks() {
    let root = tempdir().unwrap();
    let before = "/Heroic scripts/before.sh";
    let after = "/Heroic scripts/after.sh";
    let source = dusklight(
        root.path(),
        &serde_json::json!({
            "beforeLaunchScriptPath": before,
            "afterLaunchScriptPath": after,
        })
        .to_string(),
    );
    let plan = plan_native(&source, "dusk", None).unwrap();
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

    let paths = paths(root.path());
    let registered = apply_native(&paths, &source, &plan).unwrap();
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
fn apply_native_refuses_when_heroic_changed() {
    let root = tempdir().unwrap();
    let source = dusklight(root.path(), "{}");
    let plan = plan_native(&source, "dusk", None).unwrap();
    fs::write(
        source.config_dir().join("GamesConfig/dusk.json"),
        r#"{"dusk":{"useGameMode":true}}"#,
    )
    .unwrap();
    let paths = paths(root.path());
    let error = apply_native(&paths, &source, &plan).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ApplyError>(),
        Some(ApplyError::SourceChanged)
    ));
    assert!(!paths.environment_config("dusklight").exists());
}
