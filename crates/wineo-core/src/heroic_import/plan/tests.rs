use crate::test_script::write_script;
use std::fs;

use tempfile::tempdir;

use super::*;
use crate::heroic::{
    HeroicGameKind, HeroicRunner, HeroicRunnerKind, InputFingerprint, SettingsReview,
};

pub(crate) fn paths(home: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

/// A Heroic import of `Game` in a prefix at `root`, which is created.
pub(crate) fn reviewed_import(root: &Path) -> HeroicImport {
    fs::create_dir_all(root.join("drive_c")).unwrap();
    fs::write(
        root.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    HeroicImport {
        id: "game".to_owned(),
        title: "Game".to_owned(),
        prefix: root.to_owned(),
        runner: HeroicRunner {
            kind: HeroicRunnerKind::System,
            name: "wine".to_owned(),
            path: None,
            source: None,
        },
        kind: HeroicGameKind::Windows,
        exe: root.join("drive_c/game.exe"),
        args: Vec::new(),
        launcher_args: String::new(),
        cwd: Some(root.join("drive_c")),
        env: BTreeMap::new(),
        gamemode: Some(true),
        wayland: None,
        mangohud: None,
        gamescope: None,
        hooks: None,
        enable_wine_wayland: false,
        enable_hdr: false,
        dxvk: None,
        vkd3d: None,
        issues: Vec::new(),
        external_dependencies: Vec::new(),
        fingerprint: InputFingerprint {
            digest: "heroic".to_owned(),
            files: Vec::new(),
        },
        settings_review: SettingsReview::default(),
    }
}

pub(crate) fn fake_proton_runner(root: &Path, name: &str) {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("files/bin")).unwrap();
    let proton = dir.join("proton");
    write_script(&proton, "#!/bin/sh\n");
    for executable in ["wine", "wineserver"] {
        let path = dir.join("files/bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
    fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
}

pub(crate) fn fake_wine_runner(root: &Path, name: &str) -> Runner {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("bin")).unwrap();
    for executable in ["wine", "wineserver"] {
        let path = dir.join("bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
    Runners::at(root.to_owned()).resolve(name).unwrap()
}

/// Plans `imported` in `root`, named `game` and copied to `root/copy`.
pub(crate) fn planned(root: &Path, imported: HeroicImport, runner: &Runner) -> GamePlan {
    if runner.is_proton() {
        // A Proton prefix keeps its wine prefix in `pfx`.
        fs::create_dir_all(root.join("pfx/drive_c")).unwrap();
        fs::write(
            root.join("pfx/system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
    }
    GamePlan::with_runner(
        &paths(root),
        &GlobalConfig::default(),
        imported,
        Vec::new(),
        runner.name(),
        runner.clone(),
        Options {
            name: Some("game".to_owned()),
            destination: Some(root.join("copy")),
            ..Options::default()
        },
    )
    .unwrap()
}

fn issue(kind: HeroicIssueKind, field: &str, message: &str) -> HeroicIssue {
    HeroicIssue {
        kind,
        field: Some(field.to_owned()),
        message: message.to_owned(),
    }
}

fn resolution_of<'a>(plan: &'a GamePlan, field: &str) -> &'a Resolution {
    &plan
        .findings()
        .iter()
        .find(|finding| finding.issue.field.as_deref() == Some(field))
        .unwrap()
        .resolution
}

#[test]
fn wayland_and_hdr_use_the_chosen_runner_without_overriding_heroic_env() {
    let root = tempdir().unwrap();
    let runners = root.path().join("runners");
    let wine = fake_wine_runner(&runners, "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.enable_wine_wayland = true;
    imported.enable_hdr = true;
    let plan = planned(root.path(), imported, &wine);
    assert_eq!(plan.resolved().wayland, None);
    assert_eq!(plan.resolved().env.get("DXVK_HDR"), None);
    assert!(
        plan.resolved()
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("enableWineWayland"))
    );
    assert_eq!(plan.settings().wayland, None);

    fs::create_dir_all(runners.join("wine-tkg/lib/wine/x86_64-unix")).unwrap();
    fs::write(
        runners.join("wine-tkg/lib/wine/x86_64-unix/winewayland.so"),
        "",
    )
    .unwrap();
    let wine = Runners::at(runners.clone()).resolve("wine-tkg").unwrap();
    let mut imported = reviewed_import(root.path());
    imported.enable_wine_wayland = true;
    imported.enable_hdr = true;
    imported
        .env
        .insert("DXVK_HDR".to_owned(), "custom".to_owned());
    let plan = planned(root.path(), imported, &wine);
    assert_eq!(plan.resolved().wayland, Some(true));
    assert_eq!(plan.resolved().env["DXVK_HDR"], "custom");
    assert_eq!(plan.settings().wayland, Some(true));
    assert_eq!(plan.settings().env["DXVK_HDR"], "custom");

    fake_proton_runner(&runners, "proton-test");
    write_script(
        runners.join("proton-test/proton"),
        "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
    );
    let proton = Runners::at(runners).resolve("proton-test").unwrap();
    let mut imported = reviewed_import(root.path());
    imported.enable_wine_wayland = true;
    imported.enable_hdr = true;
    let plan = planned(root.path(), imported, &proton);
    assert_eq!(plan.resolved().wayland, Some(true));
    assert_eq!(plan.resolved().env["PROTON_ENABLE_HDR"], "1");
    assert!(!plan.resolved().env.contains_key("DXVK_HDR"));
    let mut imported = reviewed_import(root.path());
    imported.enable_hdr = true;
    let plan = planned(root.path(), imported, &proton);
    assert_eq!(plan.resolved().env.get("PROTON_ENABLE_HDR"), None);
}

#[test]
fn hdr_carried_with_wayland_is_not_a_finding_any_more() {
    let root = tempdir().unwrap();
    let runners = root.path().join("runners");
    fake_proton_runner(&runners, "proton-test");
    write_script(
        runners.join("proton-test/proton"),
        "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
    );
    let proton = Runners::at(runners).resolve("proton-test").unwrap();
    let mut imported = reviewed_import(root.path());
    imported.enable_wine_wayland = true;
    imported.enable_hdr = true;
    imported.issues = vec![issue(
        HeroicIssueKind::Unsupported,
        "enableHDR",
        "HDR handling is runner-specific",
    )];
    let plan = planned(root.path(), imported, &proton);
    assert_eq!(resolution_of(&plan, "enableHDR"), &Resolution::Carried);
    assert!(plan.resolved().issues.is_empty());
    // Heroic's own report is untouched.
    assert_eq!(plan.heroic().issues.len(), 1);
}

#[test]
fn review_evidence_survives_bulk_defaults_and_reaches_the_log() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.args = vec!["one word".to_owned(), "say\"hello".to_owned()];
    imported.issues = vec![
        issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed by wineo"),
        issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "not imported",
        ),
        issue(
            HeroicIssueKind::NeedsAttention,
            "autoInstallDxvkNvapi",
            "Wine NVAPI requires dxvk-nvapi DLLs; enabling variables does not install them",
        ),
    ];
    imported.settings_review.unsupported.push(ReviewItem {
        field: "wrapperOptions".to_owned(),
        detail: "ignored".to_owned(),
    });
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.skip_reason(), None);
    let warnings = plan.warnings();
    assert!(warnings.contains(&Warning::Omitted(1)), "{warnings:?}");
    assert!(warnings.contains(&Warning::Dropped(1)), "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, Warning::NvapiNotInstalled(_))),
        "{warnings:?}"
    );
    // Deciding what happens to the issues doesn't hide them from review.
    assert_eq!(plan.resolved().issues.len(), 3);
    assert_eq!(resolution_of(&plan, "WINEPREFIX"), &Resolution::Dropped);
    assert_eq!(resolution_of(&plan, "wrapperOptions"), &Resolution::Omitted);
    assert_eq!(
        resolution_of(&plan, "autoInstallDxvkNvapi"),
        &Resolution::Kept
    );

    let source =
        HeroicSource::from_config_dir_with_home(root.path().join("heroic"), root.path().to_owned());
    let log = plan.log_text(&GlobalConfig::default(), &source);
    assert!(log.contains("wrapperOptions"), "{log}");
    assert!(log.contains("WINEPREFIX"), "{log}");
    assert!(!log.contains("\u{1b}["), "{log}");
}

#[test]
fn resolved_layout_conflicts_do_not_reappear_as_current_errors() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = ["wineVersion", "winePrefix", "WINEPREFIX"]
        .into_iter()
        .map(|field| {
            issue(
                HeroicIssueKind::Conflict,
                field,
                &format!("{field} conflicts"),
            )
        })
        .collect();
    let plan = planned(root.path(), imported, &runner);
    let issues = &plan.resolved().issues;
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].field.as_deref(), Some("WINEPREFIX"));
    assert_eq!(plan.warnings(), vec![Warning::Dropped(1)]);
    assert_eq!(resolution_of(&plan, "wineVersion"), &Resolution::Covered);
    assert_eq!(resolution_of(&plan, "winePrefix"), &Resolution::Covered);
    assert_eq!(plan.blocking().count(), 0);
}

#[test]
fn covered_proton_settings_are_suppressed() {
    let root = tempdir().unwrap();
    let runners_root = root.path().join("runners");
    fake_proton_runner(&runners_root, "GE-Proton9-20");
    let proton = Runners::at(runners_root.clone())
        .resolve("GE-Proton9-20")
        .unwrap();
    let wine = fake_wine_runner(&runners_root, "wine-tkg");

    let mut imported = reviewed_import(root.path());
    imported.runner.kind = HeroicRunnerKind::Proton;
    for field in COVERED_BY_PROTON {
        imported.issues.push(issue(
            HeroicIssueKind::Unsupported,
            field,
            "Heroic component installation is not imported",
        ));
        imported.settings_review.unsupported.push(ReviewItem {
            field: field.to_owned(),
            detail: "true".to_owned(),
        });
    }
    let plan_proton = planned(root.path(), imported, &proton);
    assert!(plan_proton.resolved().issues.is_empty());
    assert!(
        plan_proton
            .resolved()
            .settings_review
            .unsupported
            .is_empty()
    );
    assert!(
        plan_proton
            .findings()
            .iter()
            .all(|finding| finding.resolution == Resolution::Covered)
    );

    // Suppression follows the chosen wineo runner, not what Heroic itself
    // recorded: a Wine choice leaves the issue for the component carry-over.
    let mut wine_import = reviewed_import(root.path());
    wine_import.runner.kind = HeroicRunnerKind::Proton;
    wine_import.issues.push(issue(
        HeroicIssueKind::Unsupported,
        "autoInstallVkd3d",
        "Heroic component installation is not imported",
    ));
    wine_import.issues.push(issue(
        HeroicIssueKind::Unsupported,
        "autoInstallDxvkNvapi",
        "Heroic component installation is not imported",
    ));
    let plan_wine = planned(root.path(), wine_import, &wine);
    assert_eq!(plan_wine.resolved().issues.len(), 1);
    assert_eq!(plan_wine.resolved().vkd3d.as_deref(), Some("latest"));
}

#[test]
fn carry_over_components_sets_dxvk_and_vkd3d_for_a_wine_runner() {
    let root = tempdir().unwrap();
    let wine = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    for field in ["autoInstallDxvk", "autoInstallVkd3d"] {
        imported.issues.push(issue(
            HeroicIssueKind::Unsupported,
            field,
            "Heroic component installation is not imported",
        ));
    }
    let plan = planned(root.path(), imported, &wine);
    assert_eq!(plan.resolved().dxvk.as_deref(), Some("latest"));
    assert_eq!(plan.resolved().vkd3d.as_deref(), Some("latest"));
    assert!(plan.resolved().issues.is_empty());
    assert_eq!(
        resolution_of(&plan, "autoInstallDxvk"),
        &Resolution::Carried
    );
    assert_eq!(plan.settings().dxvk.as_deref(), Some("latest"));
    assert_eq!(plan.settings().vkd3d.as_deref(), Some("latest"));
}

#[test]
fn carry_over_components_leaves_a_proton_choice_alone() {
    let root = tempdir().unwrap();
    let runners_root = root.path().join("runners");
    fake_proton_runner(&runners_root, "GE-Proton9-20");
    let proton = Runners::at(runners_root).resolve("GE-Proton9-20").unwrap();
    let mut imported = reviewed_import(root.path());
    imported.issues.push(issue(
        HeroicIssueKind::Unsupported,
        "wrapperOptions",
        "Heroic wrappers require manual setup",
    ));
    let plan = planned(root.path(), imported, &proton);
    assert_eq!(plan.resolved().dxvk, None);
    assert_eq!(plan.resolved().issues.len(), 1);
}

#[test]
fn settings_record_heroic_id_and_desktop_choice() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
    plan.set_desktop_enabled(true);
    plan.set_desktop_name("Game").unwrap();
    let settings = plan.settings();
    assert_eq!(settings.heroic_id.as_deref(), Some("game"));
    let target = settings.targets.values().next().unwrap();
    assert!(target.desktop);
    assert_eq!(target.name.as_deref(), Some("Game"));

    plan.set_desktop_enabled(false);
    let settings = plan.settings();
    let target = settings.targets.values().next().unwrap();
    assert!(!target.desktop);
    assert_eq!(target.name, None);
}

#[test]
fn edited_paths_expand_home_and_preserve_prefix_relative_paths() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let prefix = root.path().join("prefix");
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut plan = planned(&prefix, reviewed_import(&prefix), &runner);
    let exe = plan
        .resolve_path(&paths, "~/Games/My Game/game.exe")
        .unwrap();
    plan.set_executable(exe).unwrap();
    assert_eq!(
        plan.resolved().exe,
        root.path().join("Games/My Game/game.exe")
    );
    let cwd = plan.resolve_path(&paths, "~/Games/My Game").unwrap();
    plan.set_working_directory(Some(cwd));
    assert_eq!(plan.resolved().cwd, Some(root.path().join("Games/My Game")));
    let exe = plan
        .resolve_path(&paths, "drive_c/Program Files/game.exe")
        .unwrap();
    plan.set_executable(exe).unwrap();
    assert_eq!(
        plan.resolved().exe,
        prefix.join("drive_c/Program Files/game.exe")
    );
    let settings = plan.settings();
    let target = settings.targets.values().next().unwrap();
    // Files inside the prefix follow a copy to its destination.
    assert_eq!(
        target.exe,
        prefix
            .join("copy/drive_c/Program Files/game.exe")
            .display()
            .to_string()
    );
    assert_eq!(
        target.cwd.as_deref(),
        Some(root.path().join("Games/My Game").to_str().unwrap())
    );
    assert!(matches!(
        plan.set_executable(PathBuf::new()),
        Err(PlanError::EmptyExecutable)
    ));
}

#[test]
fn a_blank_prefix_import_gets_no_findings() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
    plan.resolve(Policy::Defaults);
    assert!(plan.findings().is_empty());
    assert_eq!(plan.skip_reason(), None);
    assert!(plan.warnings().is_empty());
}

#[test]
fn defaults_drop_conflicts_and_omit_unsupported_but_keep_attention_as_a_warning() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![
        issue(
            HeroicIssueKind::Conflict,
            "useGameMode",
            "conflicts with wineo's own setting",
        ),
        issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "not imported",
        ),
        issue(
            HeroicIssueKind::NeedsAttention,
            "wrapperOptions2",
            "review this manually",
        ),
    ];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.skip_reason(), None);
    assert_eq!(resolution_of(&plan, "useGameMode"), &Resolution::Dropped);
    assert_eq!(resolution_of(&plan, "wrapperOptions"), &Resolution::Omitted);
    assert_eq!(resolution_of(&plan, "wrapperOptions2"), &Resolution::Kept);
    assert!(
        plan.warnings()
            .iter()
            .any(|warning| matches!(warning, Warning::NeedsAttention(issues) if issues.len() == 1))
    );
}

#[test]
fn defaults_name_eac_and_battleye_specifically() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![
        issue(HeroicIssueKind::Unsupported, "eacRuntime", "not imported"),
        issue(
            HeroicIssueKind::Unsupported,
            "battlEyeRuntime",
            "not imported",
        ),
    ];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.warnings(), vec![Warning::AntiCheatNotImported]);
}

#[test]
fn defaults_reject_invalid_settings_with_their_message() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![issue(
        HeroicIssueKind::Invalid,
        "wrapperOptions",
        "cannot be parsed",
    )];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(
        plan.skip_reason(),
        Some(SkipReason::Blocked("cannot be parsed".to_owned()))
    );
}

#[test]
fn defaults_skip_invalid_launcher_args_unless_someone_corrects_them() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.launcher_args = "-a \"oops".to_owned();
    imported.issues = vec![issue(
        HeroicIssueKind::Invalid,
        "launcherArgs",
        "unbalanced quotes",
    )];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.skip_reason(), Some(SkipReason::LauncherArgs));
    assert_eq!(plan.needs_input(), Some(InputKind::LauncherArgs));
    assert_eq!(plan.blocking().count(), 0);

    assert!(matches!(
        plan.set_launcher_args("-a \"oops"),
        Err(PlanError::InvalidArguments(_))
    ));
    assert_eq!(plan.needs_input(), Some(InputKind::LauncherArgs));
    plan.set_launcher_args("-a \"fixed one\"").unwrap();
    assert_eq!(plan.needs_input(), None);
    assert_eq!(plan.resolved().args, ["-a", "fixed one"]);
    assert_eq!(plan.resolved().launcher_args, "-a \"fixed one\"");
    assert_eq!(resolution_of(&plan, "launcherArgs"), &Resolution::Fixed);
    // A fixed finding stays part of the record, and settling again keeps it.
    plan.resolve(Policy::Defaults);
    assert_eq!(resolution_of(&plan, "launcherArgs"), &Resolution::Fixed);
    assert_eq!(plan.resolved().issues.len(), 1);
}

#[test]
fn strict_blocks_everything_but_external_and_wayland_attention() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![
        issue(
            HeroicIssueKind::External,
            "external files",
            "game files or saves remain outside the imported prefix",
        ),
        issue(
            HeroicIssueKind::NeedsAttention,
            "gamescope.width",
            "review the size",
        ),
        issue(
            HeroicIssueKind::NeedsAttention,
            "targetExe",
            "executable path was relative",
        ),
        issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "not imported",
        ),
        issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed by wineo"),
    ];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Strict);
    let blocking: Vec<&str> = plan
        .blocking()
        .map(|finding| finding.issue.field.as_deref().unwrap())
        .collect();
    assert_eq!(blocking, ["targetExe", "wrapperOptions", "WINEPREFIX"]);
    assert_eq!(resolution_of(&plan, "external files"), &Resolution::Kept);
    assert_eq!(resolution_of(&plan, "gamescope.width"), &Resolution::Kept);

    // Settling by defaults again frees what strict blocked.
    plan.resolve(Policy::Defaults);
    assert_eq!(
        plan.blocking()
            .map(|f| f.issue.field.clone())
            .collect::<Vec<_>>(),
        Vec::<Option<String>>::new()
    );
}

#[test]
fn strict_blocks_launcher_args_instead_of_asking() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![issue(
        HeroicIssueKind::Invalid,
        "launcherArgs",
        "unbalanced quotes",
    )];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Strict);
    assert_eq!(plan.blocking().count(), 1);
    assert_eq!(plan.needs_input(), None);
}

#[test]
fn missing_settings_block_under_defaults_too() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![issue(HeroicIssueKind::Missing, "exe", "no executable set")];
    let mut plan = planned(root.path(), imported, &runner);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.blocking().count(), 1);
    assert_eq!(
        plan.skip_reason(),
        Some(SkipReason::Blocked("no executable set".to_owned()))
    );
}

#[test]
fn warnings_come_in_the_order_they_are_shown() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![
        issue(HeroicIssueKind::NeedsAttention, "targetExe", "relative"),
        issue(HeroicIssueKind::Unsupported, "eacRuntime", "not imported"),
        issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "not imported",
        ),
        issue(HeroicIssueKind::Unsupported, "disableUMU", "umu is off"),
        issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed"),
    ];
    let shared = vec![HeroicSharedPrefix {
        id: "other".to_owned(),
        title: "Other".to_owned(),
    }];
    let plan = GamePlan::with_runner(
        &paths(root.path()),
        &GlobalConfig::default(),
        imported,
        shared.clone(),
        runner.name(),
        runner.clone(),
        Options::default(),
    )
    .unwrap();
    assert_eq!(
        plan.warnings(),
        vec![
            Warning::SharedPrefix(shared),
            Warning::UmuDisabled("umu is off".to_owned()),
            Warning::AntiCheatNotImported,
            Warning::Omitted(1),
            Warning::Dropped(1),
            Warning::NeedsAttention(vec![issue(
                HeroicIssueKind::NeedsAttention,
                "targetExe",
                "relative"
            )]),
        ]
    );
}

#[test]
fn renaming_moves_a_default_destination_and_leaves_a_chosen_one() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let build = |destination: Option<PathBuf>| {
        GamePlan::with_runner(
            &paths,
            &global,
            reviewed_import(root.path()),
            Vec::new(),
            runner.name(),
            runner.clone(),
            Options {
                name: Some("game".to_owned()),
                destination,
                ..Options::default()
            },
        )
        .unwrap()
    };
    let mut defaulted = build(None);
    let default_root = global.prefix_root(&paths).unwrap();
    assert_eq!(defaulted.destination(), default_root.join("game"));
    defaulted.set_name(&paths, &global, "renamed").unwrap();
    assert_eq!(defaulted.name(), "renamed");
    assert_eq!(defaulted.destination(), default_root.join("renamed"));

    let mut chosen = build(Some(root.path().join("elsewhere")));
    chosen.set_name(&paths, &global, "renamed").unwrap();
    assert_eq!(chosen.destination(), root.path().join("elsewhere"));
    assert!(matches!(
        chosen.set_name(&paths, &global, "bad name"),
        Err(PlanError::InvalidName(_))
    ));
    assert_eq!(chosen.name(), "renamed");
}

#[test]
fn adopting_and_copying_switch_the_destination() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
    assert!(!plan.is_adopt());
    plan.set_adopt();
    assert!(plan.is_adopt());
    assert_eq!(plan.request().destination, None);
    assert_eq!(plan.destination(), plan.inspection().source_root);

    // Copying again without a place picks the configured prefix root.
    plan.set_copy(&paths, &global, None).unwrap();
    assert!(!plan.is_adopt());
    assert_eq!(
        plan.destination(),
        global.prefix_root(&paths).unwrap().join("game")
    );
    plan.set_copy(&paths, &global, Some(root.path().join("there")))
        .unwrap();
    assert_eq!(plan.destination(), root.path().join("there"));
}

#[test]
fn environment_entries_are_validated() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
    plan.set_environment(["A=1", "B=two=2"]).unwrap();
    assert_eq!(plan.resolved().env["B"], "two=2");
    assert!(matches!(
        plan.set_environment(["NOPE"]),
        Err(PlanError::EnvironmentFormat)
    ));
    assert!(matches!(
        plan.set_environment(["=x"]),
        Err(PlanError::EnvironmentName(_))
    ));
    let error = plan.set_environment(["WINEPREFIX=/x"]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "WINEPREFIX is managed by wineo and cannot be imported"
    );
    // A refused edit leaves the environment as it was.
    assert_eq!(plan.resolved().env.len(), 2);
}

#[test]
fn a_missing_build_is_told_apart_from_other_failures() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.dxvk = Some("latest".to_owned());
    let plan = planned(root.path(), imported, &runner);
    let missing = plan.missing_build(&paths, &global).unwrap().unwrap();
    assert_eq!(missing.component, Component::Dxvk);
    assert_eq!(missing.version, None);
    assert!(
        missing
            .message()
            .contains("Get one with: wineo download dxvk"),
        "{}",
        missing.message()
    );
    assert!(plan.check_builds(&paths, &global).is_err());

    let mut imported = reviewed_import(root.path());
    imported.dxvk = Some("dxvk-2.7".to_owned());
    let plan = planned(root.path(), imported, &runner);
    let missing = plan.missing_build(&paths, &global).unwrap().unwrap();
    assert_eq!(missing.version.as_deref(), Some("dxvk-2.7"));

    let plan = planned(root.path(), reviewed_import(root.path()), &runner);
    assert!(plan.missing_build(&paths, &global).unwrap().is_none());
    assert!(plan.check_builds(&paths, &global).is_ok());
}

#[test]
fn import_log_records_every_field_and_link() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let source =
        HeroicSource::from_config_dir_with_home(root.path().join("heroic"), root.path().to_owned());
    let mut imported = reviewed_import(root.path());
    imported.env.insert("FOO".to_owned(), "bar".to_owned());
    imported
        .env
        .insert("API_TOKEN".to_owned(), "hunter2".to_owned());
    let mut plan = planned(root.path(), imported, &runner);
    plan.set_desktop_enabled(true);
    plan.set_desktop_name("Game").unwrap();
    plan.set_runner_dependencies(vec![ExternalDependency {
        source: PathBuf::from("/prefix/a"),
        target: PathBuf::from("/heroic/tools/a"),
    }]);
    let body = plan.log_text(&GlobalConfig::default(), &source);
    assert!(body.contains("Heroic game: Game (game)"), "{body}");
    assert!(body.contains("FOO=bar"), "{body}");
    assert!(body.contains("API_TOKEN=<redacted>"), "{body}");
    assert!(!body.contains("hunter2"), "{body}");
    assert!(body.contains("desktop entry: Game"), "{body}");
    assert!(
        body.contains("runner dependency: 1 external link remains required")
            || body.contains("runner dependency: 1 external link remain required"),
        "{body}"
    );
}

#[test]
fn plans_keep_heroics_report_untouched() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![issue(
        HeroicIssueKind::NeedsAttention,
        "wineVersion",
        "runner is stored outside wineo",
    )];
    let mut plan = planned(root.path(), imported, &runner);
    plan.set_gamemode(false);
    assert_eq!(plan.heroic().issues.len(), 1);
    assert_eq!(plan.heroic().gamemode, Some(true));
    assert_eq!(plan.resolved().gamemode, Some(false));
    assert!(plan.resolved().issues.is_empty());
    assert_eq!(resolution_of(&plan, "wineVersion"), &Resolution::Covered);
}

#[test]
fn launch_setting_edits_fix_only_their_own_findings() {
    let root = tempdir().unwrap();
    let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.env.insert("OLD".to_owned(), "value".to_owned());
    imported.issues = vec![
        issue(
            HeroicIssueKind::NeedsAttention,
            "enviromentOptions[0]",
            "review env",
        ),
        issue(
            HeroicIssueKind::NeedsAttention,
            "gamescope.width",
            "review width",
        ),
        issue(HeroicIssueKind::NeedsAttention, "enableHDR", "review HDR"),
    ];
    let mut plan = planned(root.path(), imported.clone(), &runner);
    plan.resolve(Policy::Defaults);
    plan.set_environment(["NEW=other"]).unwrap();
    plan.set_gamescope(Some(GamescopeSetting::On(Default::default())));
    plan.set_mangohud(true);
    assert_eq!(plan.heroic().env, imported.env);
    assert_eq!(plan.heroic().issues, imported.issues);
    assert_eq!(plan.resolved().env["NEW"], "other");
    assert_eq!(plan.resolved().mangohud, Some(true));
    assert_eq!(
        resolution_of(&plan, "enviromentOptions[0]"),
        &Resolution::Fixed
    );
    assert_eq!(resolution_of(&plan, "gamescope.width"), &Resolution::Fixed);
    assert_ne!(resolution_of(&plan, "enableHDR"), &Resolution::Fixed);
    assert!(plan.set_environment(["BROKEN"]).is_err());
    assert_eq!(plan.resolved().env["NEW"], "other");
}

#[test]
fn explicit_wayland_and_hdr_edits_follow_the_selected_runner() {
    let root = tempdir().unwrap();
    let runners = root.path().join("runners");
    fake_proton_runner(&runners, "proton-test");
    write_script(
        runners.join("proton-test/proton"),
        "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
    );
    let proton = Runners::at(runners).resolve("proton-test").unwrap();
    let imported = reviewed_import(root.path());
    let mut plan = planned(root.path(), imported, &proton);
    plan.set_wayland(true).unwrap();
    plan.set_hdr(true).unwrap();
    assert_eq!(plan.settings().wayland, Some(true));
    assert_eq!(plan.settings().env["PROTON_ENABLE_HDR"], "1");
    assert!(!plan.heroic().enable_wine_wayland);
    assert!(!plan.heroic().enable_hdr);
    plan.set_wayland(false).unwrap();
    assert_eq!(plan.settings().wayland, Some(false));
    assert!(!plan.settings().env.contains_key("PROTON_ENABLE_HDR"));
    assert!(!plan.resolved().enable_hdr);
}
