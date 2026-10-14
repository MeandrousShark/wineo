use super::test_access::access;
use crate::test_script::write_script;
use std::collections::VecDeque;
use std::fs;

use std::path::PathBuf;

use tempfile::tempdir;
use wineo_core::heroic::{HeroicRunner, InputFingerprint};
use wineo_core::heroic_import::{PendingGame, Resolution};
use wineo_core::import;
use wineo_core::runner::Runner;

use super::bulk::{
    Bulk, BulkRow, RowStatus, bulk_art, bulk_desktop_choice, bulk_rows, import_one,
    import_progress, render_bulk_table, resolve_pending, resolve_pending_with,
    shared_ready_warning,
};
use super::review::{edit_executable, edit_working_directory};
use super::runner::ensure_components_available_with;
use super::summary::{link_lines, warnings_line};
use super::*;
use crate::commands::art::fake::Fake;
use crate::prompt::Script;
use crate::text;
use std::collections::BTreeMap;
use std::path::Path;
use wineo_core::components::Component;
use wineo_core::heroic::{HeroicImport, HeroicIssueKind, HeroicRunnerKind, SettingsReview};
use wineo_core::heroic_import::{Entry, Game, LibraryOptions, LibraryPlan, plan_library};
use wineo_core::import::ExternalDependency;
use wineo_core::runners::Runners;

fn paths(home: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn reviewed_import(root: &Path) -> HeroicImport {
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

/// A global config whose runners are the ones a test writes in `root/runners`.
fn global_with_runners(root: &Path) -> GlobalConfig {
    GlobalConfig {
        defaults: wineo_core::config::Defaults {
            runner_root: Some(root.join("runners").display().to_string()),
            ..wineo_core::config::Defaults::default()
        },
        ..GlobalConfig::default()
    }
}

/// Plans `imported` against a fake wine runner, named `game` and copied to
/// `root/copy`.
fn plan_for(root: &Path, imported: HeroicImport) -> GamePlan {
    let runners = root.join("runners");
    if !runners.join("wine-tkg").exists() {
        fake_wine_runner(&runners, "wine-tkg");
    }
    GamePlan::new(
        &paths(root),
        &global_with_runners(root),
        imported,
        Vec::new(),
        "wine-tkg",
        Options {
            name: Some("game".to_owned()),
            destination: Some(root.join("copy")),
            ..Options::default()
        },
    )
    .unwrap()
}

fn heroic_source(root: &Path) -> HeroicSource {
    HeroicSource::from_config_dir_with_home(root.join("heroic"), root.to_owned())
}

fn fake_proton_runner(root: &Path, name: &str) {
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

fn fake_wine_runner(root: &Path, name: &str) -> Runner {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("bin")).unwrap();
    for executable in ["wine", "wineserver"] {
        let path = dir.join("bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
    Runners::at(root.to_owned()).resolve(name).unwrap()
}

#[test]
fn final_review_edits_executable_before_import() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = global_with_runners(root.path());
    let source = heroic_source(root.path());
    let mut plan = plan_for(root.path(), reviewed_import(root.path()));
    plan.set_desktop_enabled(true);
    let mut prompt = Prompt::new(
        Script(VecDeque::from([
            "1".to_owned(),
            "drive_c/edited.exe".to_owned(),
            "".to_owned(),
        ])),
        Vec::new(),
    );
    final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        &mut ArtChoice::off(),
        &mut prompt,
    )
    .unwrap();
    assert_eq!(plan.resolved().exe, root.path().join("drive_c/edited.exe"));
}

#[test]
fn edited_paths_expand_home_and_preserve_prefix_relative_paths() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let prefix = root.path().join("prefix");
    let mut plan = plan_for(&prefix, reviewed_import(&prefix));
    let mut prompt = Prompt::new(Script::new(&["\"~/Games/My Game/game.exe\""]), Vec::new());
    edit_executable(&paths, &mut plan, &mut prompt).unwrap();
    assert_eq!(
        plan.resolved().exe,
        root.path().join("Games/My Game/game.exe")
    );
    let mut prompt = Prompt::new(Script::new(&["~/Games/My\\ Game"]), Vec::new());
    edit_working_directory(&paths, &mut plan, &mut prompt).unwrap();
    assert_eq!(plan.resolved().cwd, Some(root.path().join("Games/My Game")));

    let mut prompt = Prompt::new(
        Script::new(&["drive_c/Program\\ Files/game.exe"]),
        Vec::new(),
    );
    edit_executable(&paths, &mut plan, &mut prompt).unwrap();
    assert_eq!(
        plan.resolved().exe,
        prefix.join("drive_c/Program Files/game.exe")
    );

    let absolute = root.path().join("External Game/game.exe");
    let edited = plan
        .resolve_path(
            &paths,
            &picker::clean_path(&format!("'{}'", absolute.display())),
        )
        .unwrap();
    assert_eq!(edited, absolute);
}

#[test]
fn review_evidence_survives_bulk_defaults_and_reaches_output_and_log() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let source = heroic_source(root.path());
    let mut imported = reviewed_import(root.path());
    imported.args = vec!["one word".to_owned(), "say\"hello".to_owned()];
    imported.issues = vec![
        HeroicIssue {
            kind: HeroicIssueKind::Conflict,
            field: Some("WINEPREFIX".to_owned()),
            message: "managed by wineo".to_owned(),
        },
        HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("wrapperOptions".to_owned()),
            message: "not imported".to_owned(),
        },
        HeroicIssue {
            kind: HeroicIssueKind::NeedsAttention,
            field: Some("autoInstallDxvkNvapi".to_owned()),
            message:
                "Wine NVAPI requires dxvk-nvapi DLLs; enabling variables does not install them"
                    .to_owned(),
        },
    ];
    imported
        .settings_review
        .unsupported
        .push(wineo_core::heroic::ReviewItem {
            field: "wrapperOptions".to_owned(),
            detail: "ignored".to_owned(),
        });
    let mut plan = plan_for(root.path(), imported);
    plan.resolve(Policy::Defaults);
    assert_eq!(plan.skip_reason(), None);
    let warning = warnings_line(&plan.warnings(), false).unwrap();
    assert!(warning.contains("unsupported setting omitted"), "{warning}");
    assert!(warning.contains("managed conflict dropped"), "{warning}");
    assert!(warning.contains("Wine NVAPI requires"), "{warning}");

    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    show_game_summary(&mut prompt, &paths, plan.resolved(), true).unwrap();
    let output = String::from_utf8_lossy(prompt.output());
    assert!(output.contains("wrapperOptions"), "{output}");
    assert!(output.contains("WINEPREFIX"), "{output}");
    assert!(output.contains("\"one word\""), "{output}");
    assert!(output.contains("say\\\"hello"), "{output}");
    let log = plan.log_text(&global, &source);
    assert!(log.contains("wrapperOptions"), "{log}");
    assert!(log.contains("WINEPREFIX"), "{log}");
    assert!(!log.contains("\u{1b}["), "{log}");
}

#[test]
fn resolved_layout_conflicts_do_not_reappear_as_current_errors() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let mut imported = reviewed_import(root.path());
    imported.issues = ["wineVersion", "winePrefix", "WINEPREFIX"]
        .into_iter()
        .map(|field| HeroicIssue {
            kind: HeroicIssueKind::Conflict,
            field: Some(field.to_owned()),
            message: format!("{field} conflicts"),
        })
        .collect();
    let plan = plan_for(root.path(), imported);
    assert_eq!(
        warnings_line(&plan.warnings(), false).as_deref(),
        Some("1 managed conflict dropped")
    );
    let mut output = Prompt::new(Script::new(&[]), Vec::new());
    show_game_summary(&mut output, &paths, plan.resolved(), true).unwrap();
    let text = String::from_utf8_lossy(output.output());
    assert!(!text.contains("wineVersion conflicts"), "{text}");
    assert!(!text.contains("winePrefix conflicts"), "{text}");
    assert!(text.contains("WINEPREFIX is dropped"), "{text}");
}

#[test]
fn import_progress_flushes_before_blocked_copy_and_completes_same_line() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    struct VisibleWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        flushes: Arc<AtomicUsize>,
    }
    impl Write for VisibleWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let root = tempdir().unwrap();
    let source_root = root.path().join("source-runners");
    fake_wine_runner(&source_root, "wine-tkg");
    let reviewed = import::inspect_runner_copy(source_root.join("wine-tkg")).unwrap();
    let destination_root = root.path().join("copied-runners");
    let installed = destination_root.join("wine-copy");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let flushes = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer = VisibleWriter {
        bytes: bytes.clone(),
        flushes: flushes.clone(),
    };
    let worker = std::thread::spawn(move || {
        let mut writer = writer;
        import_progress(&mut writer, "hades", || {
            let cancelled = wineo_core::cancel::Token::new();
            let mut paused = false;
            import::copy_runner_reviewed(
                &reviewed,
                destination_root,
                "wine-copy",
                &cancelled,
                |progress| {
                    if !paused && matches!(progress, import::ImportProgress::Copied { .. }) {
                        paused = true;
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                },
            )
        })
        .unwrap()
        .unwrap();
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(&*bytes.lock().unwrap(), b"Importing hades...");
    assert_eq!(flushes.load(Ordering::SeqCst), 1);
    assert!(!installed.exists());
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(&*bytes.lock().unwrap(), b"Importing hades... done\n");
    assert!(installed.join("bin/wine").is_file());

    let mut failed = Vec::new();
    assert!(
        import_progress(&mut failed, "alpha", || -> anyhow::Result<()> {
            anyhow::bail!("copy failed")
        })
        .unwrap()
        .is_err()
    );
    assert_eq!(failed, b"Importing alpha... failed: copy failed\n");
}

#[test]
fn final_review_cancel_returns_canceled_without_changes() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = global_with_runners(root.path());
    let source = heroic_source(root.path());
    let mut plan = plan_for(root.path(), reviewed_import(root.path()));
    plan.set_desktop_enabled(true);
    let original = plan.resolved().exe.clone();
    let mut prompt = Prompt::new(Script::new(&["q"]), Vec::new());
    let error = final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        &mut ArtChoice::off(),
        &mut prompt,
    )
    .unwrap_err();
    assert!(error.is::<Canceled>());
    assert_eq!(plan.resolved().exe, original);
}

#[test]
fn final_review_bare_enter_imports_without_editing() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = global_with_runners(root.path());
    let source = heroic_source(root.path());
    let mut plan = plan_for(root.path(), reviewed_import(root.path()));
    plan.set_desktop_enabled(true);
    let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
    final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        &mut ArtChoice::off(),
        &mut prompt,
    )
    .unwrap();
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    assert!(printed.contains("Game -> game"), "{printed}");
    assert!(printed.contains("  8) desktop      yes"), "{printed}");
    assert!(
        printed.contains(
            "Close Heroic and the game first.\n\nPress Enter to import, or pick a number. Esc to go back, Ctrl+C to cancel.\nChoice: "
        ),
        "{printed}"
    );
}

#[test]
fn final_review_toggles_the_desktop_entry() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = global_with_runners(root.path());
    let source = heroic_source(root.path());
    let mut plan = plan_for(root.path(), reviewed_import(root.path()));
    plan.set_desktop_enabled(true);
    let mut prompt = Prompt::new(Script::new(&["8", "n", ""]), Vec::new());
    final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        &mut ArtChoice::off(),
        &mut prompt,
    )
    .unwrap();
    assert!(!plan.desktop().enabled);
}

#[test]
fn game_summary_merges_eac_and_battleye_into_one_warning() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let mut imported = reviewed_import(root.path());
    imported.issues.push(HeroicIssue {
        kind: HeroicIssueKind::Unsupported,
        field: Some("eacRuntime".to_owned()),
        message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
    });
    imported.issues.push(HeroicIssue {
        kind: HeroicIssueKind::Unsupported,
        field: Some("battlEyeRuntime".to_owned()),
        message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
    });
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    show_game_summary(&mut prompt, &paths, &imported, false).unwrap();
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    assert_eq!(printed.matches("aren't imported").count(), 1, "{printed}");
    assert!(
        printed.contains("warning      EAC and BattlEye runtimes aren't imported"),
        "{printed}"
    );
}

#[test]
fn review_items_are_summary_warnings_not_prompts() {
    let root = tempdir().unwrap();
    let mut imported = reviewed_import(root.path());
    imported.issues = vec![
        HeroicIssue {
            kind: HeroicIssueKind::NeedsAttention,
            field: Some("targetExe".to_owned()),
            message: "executable path was relative".to_owned(),
        },
        HeroicIssue {
            kind: HeroicIssueKind::External,
            field: Some("external files".to_owned()),
            message: "game files or saves remain outside the imported prefix".to_owned(),
        },
        HeroicIssue {
            kind: HeroicIssueKind::Conflict,
            field: Some("WINEPREFIX".to_owned()),
            message: "Heroic setting conflicts with a wineo-managed variable".to_owned(),
        },
        HeroicIssue {
            kind: HeroicIssueKind::Unsupported,
            field: Some("gamescope".to_owned()),
            message: "gamescope is not imported".to_owned(),
        },
    ];
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    show_game_summary(&mut prompt, &paths(root.path()), &imported, false).unwrap();
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    assert!(
        printed.contains("executable path was relative"),
        "{printed}"
    );
    assert!(!printed.contains("remain outside"), "{printed}");
    assert!(
        printed.contains("WINEPREFIX is dropped, wineo manages it"),
        "{printed}"
    );
    assert!(
        printed.contains("1 more Heroic setting is not imported"),
        "{printed}"
    );
    assert!(!printed.contains("error"), "{printed}");

    // Reviewing settles them without asking anything, since the script
    // has no answers to give.
    let mut plan = plan_for(root.path(), imported);
    review_settings(&mut prompt, &mut plan).unwrap();
    assert_eq!(plan.blocking().count(), 0);
    assert_eq!(plan.needs_input(), None);
    let settled: Vec<_> = plan
        .findings()
        .iter()
        .map(|finding| finding.resolution.clone())
        .collect();
    assert_eq!(
        settled,
        [
            Resolution::Kept,
            Resolution::Kept,
            Resolution::Dropped,
            Resolution::Omitted
        ]
    );
}

#[test]
fn ensure_components_available_passes_through_when_nothing_is_missing() {
    let root = tempdir().unwrap();
    let plan = plan_for(root.path(), reviewed_import(root.path()));
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    ensure_components_available(
        &paths(root.path()),
        &global_with_runners(root.path()),
        &plan,
        &mut prompt,
    )
    .unwrap();
}

#[test]
fn ensure_components_available_declining_leaves_the_usual_hinted_error() {
    let root = tempdir().unwrap();
    let mut imported = reviewed_import(root.path());
    imported.dxvk = Some("latest".to_owned());
    let plan = plan_for(root.path(), imported);
    let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
    let error = ensure_components_available(
        &paths(root.path()),
        &global_with_runners(root.path()),
        &plan,
        &mut prompt,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("no dxvk builds are installed"),
        "{message}"
    );
    assert!(
        message.contains("Get one with: wineo download dxvk"),
        "{message}"
    );
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    assert!(
        printed.contains("No matching DXVK build is installed. Download the latest release?"),
        "{printed}"
    );
}

#[test]
fn link_lines_group_by_heroics_runner_and_hide_dosdevices() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let config_dir = root.path().join("heroic");
    let source =
        HeroicSource::from_config_dir_with_home(config_dir.clone(), root.path().to_owned());
    let runner_dir = config_dir.join("tools/proton/GE-Proton11-1");
    let deps = vec![
        ExternalDependency {
            source: PathBuf::from("/prefix/dosdevices/z:"),
            target: PathBuf::from("/"),
        },
        ExternalDependency {
            source: PathBuf::from("/prefix/drive_c/windows/fonts/a.ttf"),
            target: runner_dir.join("files/share/fonts/a.ttf"),
        },
        ExternalDependency {
            source: PathBuf::from("/prefix/drive_c/windows/fonts/b.ttf"),
            target: runner_dir.join("files/share/fonts/b.ttf"),
        },
    ];
    let lines = link_lines(&paths, &source, &deps, false);
    assert_eq!(
        lines,
        vec!["2 prefix files point into Heroic's GE-Proton11-1"]
    );
    let verbose_lines = link_lines(&paths, &source, &deps, true);
    assert_eq!(verbose_lines.len(), 2);
}

#[test]
fn game_summary_shows_aligned_rows_for_a_typical_proton_game() {
    let root = tempdir().unwrap();
    let root = root.path();
    let paths = paths(root);
    fs::create_dir_all(root.join("drive_c")).unwrap();
    fs::write(
        root.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    let exe = root.join("Games/Palworld/Palworld.exe");
    let mut imported = HeroicImport {
        id: "kZXLzzKAFWBP4dpRatpPHu".to_owned(),
        title: "Palworld".to_owned(),
        prefix: root.join("Games/Heroic/Prefixes/default/Palworld"),
        runner: HeroicRunner {
            kind: HeroicRunnerKind::Proton,
            name: "Proton-CachyOS-latest".to_owned(),
            path: Some(root.join(".config/heroic/tools/proton/Proton-CachyOS-latest/proton")),
            source: Some(root.join(".config/heroic/tools/proton/Proton-CachyOS-latest")),
        },
        kind: HeroicGameKind::Windows,
        exe: exe.clone(),
        args: Vec::new(),
        launcher_args: String::new(),
        cwd: exe.parent().map(Path::to_owned),
        env: BTreeMap::from([
            ("DXVK_NVAPI_ALLOW_OTHER_DRIVERS".to_owned(), "1".to_owned()),
            ("PROTON_ENABLE_NVAPI".to_owned(), "1".to_owned()),
            ("PROTON_ENABLE_WAYLAND".to_owned(), "1".to_owned()),
        ]),
        gamemode: Some(true),
        wayland: None,
        mangohud: None,
        gamescope: None,
        hooks: None,
        enable_wine_wayland: false,
        enable_hdr: false,
        dxvk: None,
        vkd3d: None,
        issues: vec![
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("eacRuntime".to_owned()),
                message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Unsupported,
                field: Some("battlEyeRuntime".to_owned()),
                message: "Heroic-managed anti-cheat runtime is not imported".to_owned(),
            },
            HeroicIssue {
                kind: HeroicIssueKind::Conflict,
                field: Some("winePrefix".to_owned()),
                message: "prefix contains both direct and nested Proton registry markers"
                    .to_owned(),
            },
        ],
        external_dependencies: vec![exe.clone()],
        fingerprint: InputFingerprint {
            digest: "heroic".to_owned(),
            files: Vec::new(),
        },
        settings_review: SettingsReview::default(),
    };
    imported.settings_review.carried.extend([
        wineo_core::heroic::ReviewItem {
            field: "before hook".to_owned(),
            detail: "/Heroic scripts/before\nreview: 'game'.sh".to_owned(),
        },
        wineo_core::heroic::ReviewItem {
            field: "after hook".to_owned(),
            detail: "/Heroic scripts/after.sh".to_owned(),
        },
    ]);
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    show_game_summary(&mut prompt, &paths, &imported, false).unwrap();
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    println!("{printed}");
    assert!(printed.contains("Palworld (Heroic kZXLzzKAFWBP4dpRatpPHu)"));
    assert!(printed.contains("runner       Proton-CachyOS-latest (Heroic's copy)"));
    assert!(printed.contains("gamemode     on"));
    assert!(printed.contains("before hook  /Heroic scripts/before\\nreview: 'game'.sh"));
    assert!(printed.contains("after hook   /Heroic scripts/after.sh"));
    assert!(printed.contains("warning      EAC and BattlEye runtimes aren't imported"));
    assert!(!printed.contains("registry markers"), "{printed}");
}

#[test]
fn final_review_matches_the_palworld_example_shape() {
    let root = tempdir().unwrap();
    let root = root.path();
    let paths = paths(root);
    let runner_root = root.join("runners");
    let runner_name = "proton-cachyos-11.0-20260703-slr-x86_64";
    fake_proton_runner(&runner_root, runner_name);
    let global = GlobalConfig {
        defaults: wineo_core::config::Defaults {
            runner_root: Some(runner_root.display().to_string()),
            ..wineo_core::config::Defaults::default()
        },
        ..GlobalConfig::default()
    };
    let config_dir = root.join(".config/heroic");
    let source = HeroicSource::from_config_dir_with_home(config_dir.clone(), root.to_owned());
    let compatdata = root.join("Games/Heroic/Prefixes/default/Palworld");
    fs::create_dir_all(compatdata.join("pfx/drive_c")).unwrap();
    fs::write(
        compatdata.join("pfx/system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    // Fonts that link into Heroic's own copy of a runner.
    let runner_dir = config_dir.join("tools/proton/GE-Proton11-1");
    fs::create_dir_all(compatdata.join("pfx/drive_c/windows")).unwrap();
    for index in 0..24 {
        std::os::unix::fs::symlink(
            runner_dir
                .join("files/share/fonts")
                .join(format!("font{index}.ttf")),
            compatdata
                .join("pfx/drive_c/windows")
                .join(format!("font{index}.ttf")),
        )
        .unwrap();
    }
    let exe = root.join("Games/Palworld/Palworld.exe");
    let imported = HeroicImport {
        id: "kZXLzzKAFWBP4dpRatpPHu".to_owned(),
        title: "Palworld".to_owned(),
        prefix: compatdata.clone(),
        runner: HeroicRunner {
            kind: HeroicRunnerKind::Proton,
            name: "Proton-CachyOS-latest".to_owned(),
            path: Some(config_dir.join("tools/proton/Proton-CachyOS-latest/proton")),
            source: Some(config_dir.join("tools/proton/Proton-CachyOS-latest")),
        },
        kind: HeroicGameKind::Windows,
        exe: exe.clone(),
        args: Vec::new(),
        launcher_args: String::new(),
        cwd: exe.parent().map(Path::to_owned),
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
        external_dependencies: vec![exe.clone()],
        fingerprint: InputFingerprint {
            digest: "heroic".to_owned(),
            files: Vec::new(),
        },
        settings_review: SettingsReview::default(),
    };
    let mut plan = GamePlan::new(
        &paths,
        &global,
        imported,
        Vec::new(),
        runner_name,
        Options {
            name: Some("palworld".to_owned()),
            destination: Some(root.join(".local/share/wineo/prefixes/palworld")),
            ..Options::default()
        },
    )
    .unwrap();
    plan.set_desktop_enabled(true);
    plan.set_desktop_name("palworld").unwrap();
    let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
    final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        &mut ArtChoice::off(),
        &mut prompt,
    )
    .unwrap();
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    println!("{printed}");
    assert!(printed.contains("Palworld -> palworld"));
    assert!(printed.contains("links     24 prefix files point into Heroic's GE-Proton11-1"));
    assert!(printed.contains("8) desktop      yes"));
}

// -- --all (bulk import) --

#[test]
fn bulk_desktop_choice_follows_desktop_and_yes_flags() {
    assert_eq!(bulk_desktop_choice(true, true), Some(true));
    assert_eq!(bulk_desktop_choice(true, false), Some(true));
    assert_eq!(bulk_desktop_choice(false, true), Some(false));
    assert_eq!(bulk_desktop_choice(false, false), None);
}

#[test]
fn render_bulk_table_matches_the_plan_example_shape() {
    let rows = vec![
        BulkRow {
            name: "palworld".to_owned(),
            title: "Palworld".to_owned(),
            status: RowStatus::Ready { warn: None },
            launch_scripts: false,
        },
        BulkRow {
            name: "hades".to_owned(),
            title: "Hades".to_owned(),
            status: RowStatus::Ready {
                warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
            },
            launch_scripts: false,
        },
        BulkRow {
            name: "celeste".to_owned(),
            title: "Celeste".to_owned(),
            status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
            launch_scripts: false,
        },
    ];
    let already = vec!["zomboid".to_owned()];
    let text = render_bulk_table("GE-Proton11-1", "copy", &rows, &already);
    let expected = format!(
        "Import 3 Heroic games with GE-Proton11-1:\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\nAlready imported: zomboid",
        "palworld",
        "Palworld",
        "copy",
        "hades",
        "Hades",
        "copy   warn: EAC/BattlEye runtimes aren't imported",
        "celeste",
        "Celeste",
        "skip: Wine runner, needs --runner",
    );
    assert_eq!(text, expected);
}

#[test]
fn render_bulk_table_marks_windows_and_native_games_with_scripts() {
    let rows = vec![
        BulkRow {
            name: "palworld".to_owned(),
            title: "Palworld".to_owned(),
            status: RowStatus::Ready { warn: None },
            launch_scripts: true,
        },
        BulkRow {
            name: "native".to_owned(),
            title: "Native Game".to_owned(),
            status: RowStatus::ReadyNative { warn: None },
            launch_scripts: true,
        },
    ];
    let text = render_bulk_table("", "copy", &rows, &[]);
    assert!(
        text.lines()
            .any(|line| line.ends_with("copy     runs launch scripts")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.ends_with("native   runs launch scripts")),
        "{text}"
    );
}

#[test]
fn control_characters_are_escaped_only_for_hook_display() {
    assert_eq!(
        wineo_core::heroic::display_control_escaped("/Heroic scripts/a\nreview\r\u{1b}.sh"),
        "/Heroic scripts/a\\nreview\\r\\u{1b}.sh"
    );
    assert_eq!(
        wineo_core::heroic::display_control_escaped("/Heroic scripts/a 'quoted' $dollar; ok.sh"),
        "/Heroic scripts/a 'quoted' $dollar; ok.sh"
    );
}

#[test]
fn render_bulk_table_pluralizes_with_and_without_a_runner() {
    for count in [0, 1, 2] {
        for runner in ["", "test-wine"] {
            let rows: Vec<_> = (0..count)
                .map(|index| BulkRow {
                    name: format!("game{index}"),
                    title: format!("Game {index}"),
                    status: RowStatus::ReadyNative { warn: None },
                    launch_scripts: false,
                })
                .collect();
            let noun = text::plural(count, "game", "games");
            let suffix = if runner.is_empty() {
                String::new()
            } else {
                format!(" with {runner}")
            };
            let text = render_bulk_table(runner, "copy", &rows, &[]);
            assert_eq!(
                text.lines().next().unwrap(),
                format!("Import {count} Heroic {noun}{suffix}:")
            );
        }
    }
}

#[test]
fn render_bulk_table_lines_up_accented_and_wide_titles() {
    let row = |name: &str, title: &str| BulkRow {
        name: name.to_owned(),
        title: title.to_owned(),
        status: RowStatus::ReadyNative { warn: None },
        launch_scripts: false,
    };
    let rows = vec![
        row("cafe", "Caf\u{e9}"),
        row("genshin", "\u{539f}\u{795e}"),
        row("dwarf", "Dwarf"),
    ];
    let text = render_bulk_table("", "copy", &rows, &[]);
    // Each CJK character takes two columns, so the title takes four.
    assert_eq!(
        text.lines().skip(1).collect::<Vec<_>>(),
        [
            "  cafe     Caf\u{e9}   native",
            "  genshin  \u{539f}\u{795e}   native",
            "  dwarf    Dwarf  native",
        ]
    );
}

#[test]
fn render_bulk_table_collapses_a_warning_every_ready_row_shares() {
    let rows = vec![
        BulkRow {
            name: "palworld".to_owned(),
            title: "Palworld".to_owned(),
            status: RowStatus::Ready {
                warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
            },
            launch_scripts: false,
        },
        BulkRow {
            name: "hades".to_owned(),
            title: "Hades".to_owned(),
            status: RowStatus::Ready {
                warn: Some("EAC/BattlEye runtimes aren't imported".to_owned()),
            },
            launch_scripts: false,
        },
        BulkRow {
            name: "celeste".to_owned(),
            title: "Celeste".to_owned(),
            status: RowStatus::Skipped("Wine runner, needs --runner".to_owned()),
            launch_scripts: false,
        },
    ];
    let text = render_bulk_table("GE-Proton11-1", "copy", &rows, &[]);
    let expected = format!(
        "Import 3 Heroic games with GE-Proton11-1:\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\n  {:8}  {:8}  {}\nwarn: EAC/BattlEye runtimes aren't imported",
        "palworld",
        "Palworld",
        "copy",
        "hades",
        "Hades",
        "copy",
        "celeste",
        "Celeste",
        "skip: Wine runner, needs --runner",
    );
    assert_eq!(text, expected);
}

#[test]
fn shared_ready_warning_requires_every_ready_row_to_agree() {
    let same = |name: &str, warn: Option<&str>| BulkRow {
        name: name.to_owned(),
        title: name.to_owned(),
        status: RowStatus::Ready {
            warn: warn.map(str::to_owned),
        },
        launch_scripts: false,
    };
    assert_eq!(
        shared_ready_warning(&[same("a", Some("x")), same("b", Some("x"))]),
        Some("x")
    );
    assert_eq!(
        shared_ready_warning(&[same("a", Some("x")), same("b", Some("y"))]),
        None
    );
    assert_eq!(
        shared_ready_warning(&[same("a", Some("x")), same("b", None)]),
        None
    );
    assert_eq!(shared_ready_warning(&[same("a", None)]), None);
    assert_eq!(
        shared_ready_warning(&[BulkRow {
            name: "a".to_owned(),
            title: "a".to_owned(),
            status: RowStatus::Skipped("nope".to_owned()),
            launch_scripts: false,
        }]),
        None
    );
}

pub(super) fn pending_game_wanting(
    root: &Path,
    component: Component,
    reason: &str,
    dxvk: Option<&str>,
    vkd3d: Option<&str>,
    version: Option<&str>,
) -> Entry {
    let mut imported = reviewed_import(root);
    imported.dxvk = dxvk.map(str::to_owned);
    imported.vkd3d = vkd3d.map(str::to_owned);
    Entry::Pending(PendingGame::new(
        plan_for(root, imported),
        component,
        version.map(str::to_owned),
        reason.to_owned(),
    ))
}

/// A plan whose only games are waiting on builds.
fn waiting(games: Vec<Entry>) -> LibraryPlan {
    LibraryPlan::from_entries(Some("wine-tkg".to_owned()), Vec::new(), games)
}

fn pending_game(root: &Path, component: Component, reason: &str) -> Entry {
    pending_game_wanting(
        root,
        component,
        reason,
        Some("latest"),
        (component == Component::Vkd3d).then_some("latest"),
        None,
    )
}

fn install_component_fixture(
    paths: &Paths,
    global: &GlobalConfig,
    component: Component,
    name: &str,
) {
    let root = wineo_core::components::Builds::new(paths, global, component)
        .unwrap()
        .root()
        .to_owned();
    let (x64, x32) = component.arch_dirs();
    let marker = if component == Component::Dxvk {
        "dxgi.dll"
    } else {
        "d3d12.dll"
    };
    for arch in [x64, x32] {
        let dir = root.join(name).join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(marker), b"fixture").unwrap();
    }
}

#[test]
fn guided_accepts_exact_pinned_component_and_resumes() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let mut imported = reviewed_import(root.path());
    imported.dxvk = Some("dxvk-2.7".to_owned());
    let plan = plan_for(root.path(), imported);
    let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
    let mut requests = Vec::new();
    ensure_components_available_with(&paths, &global, &plan, &mut prompt, |component, version| {
        requests.push((component, version.map(str::to_owned)));
        install_component_fixture(&paths, &global, component, version.unwrap());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        requests,
        vec![(Component::Dxvk, Some("dxvk-2.7".to_owned()))]
    );
    let output = String::from_utf8_lossy(prompt.output());
    assert!(output.contains("Download that exact version?"), "{output}");
    assert_eq!(plan.wanted().dxvk.as_deref(), Some("dxvk-2.7"));
}

#[test]
fn guided_accepts_both_components_in_dependency_order() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let mut imported = reviewed_import(root.path());
    imported.dxvk = Some("latest".to_owned());
    imported.vkd3d = Some("latest".to_owned());
    let plan = plan_for(root.path(), imported);
    let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
    let mut requests = Vec::new();
    ensure_components_available_with(&paths, &global, &plan, &mut prompt, |component, version| {
        requests.push((component, version.map(str::to_owned)));
        let name = if component == Component::Dxvk {
            "dxvk-2.7"
        } else {
            "vkd3d-proton-2.14"
        };
        install_component_fixture(&paths, &global, component, name);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        requests,
        vec![(Component::Dxvk, None), (Component::Vkd3d, None)]
    );
}

#[test]
fn bulk_accepts_one_component_prompt_for_shared_pin_and_resumes() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let pinned = || {
        pending_game_wanting(
            root.path(),
            Component::Dxvk,
            "missing",
            Some("dxvk-2.7"),
            None,
            Some("dxvk-2.7"),
        )
    };
    let mut library = waiting(vec![pinned(), pinned()]);
    let mut prompt = Prompt::new(Script::new(&["y"]), Vec::new());
    let mut requests = Vec::new();
    resolve_pending_with(
        &paths,
        &global,
        &mut library,
        false,
        Some(&mut prompt),
        |component, version| {
            requests.push((component, version.map(str::to_owned)));
            install_component_fixture(&paths, &global, component, version.unwrap());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        requests,
        vec![(Component::Dxvk, Some("dxvk-2.7".to_owned()))]
    );
    let rows = bulk_rows(&library);
    assert_eq!(library.into_ready().len(), 2);
    assert!(
        rows.iter()
            .all(|row| matches!(row.status, RowStatus::Ready { .. }))
    );
    assert_eq!(
        String::from_utf8_lossy(prompt.output())
            .matches("Download the exact requested version?")
            .count(),
        1
    );
}

#[test]
fn bulk_downloads_dxvk_then_vkd3d_and_resumes() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let game = pending_game_wanting(
        root.path(),
        Component::Dxvk,
        "dxvk missing",
        Some("latest"),
        Some("latest"),
        None,
    );
    let mut library = waiting(vec![game]);
    let mut prompt = Prompt::new(Script::new(&["y", "y"]), Vec::new());
    let mut requested = Vec::new();
    resolve_pending_with(
        &paths,
        &global,
        &mut library,
        false,
        Some(&mut prompt),
        |component, version| {
            requested.push((component, version.map(str::to_owned)));
            let name = if component == Component::Dxvk {
                "dxvk-2.7"
            } else {
                "vkd3d-proton-2.14"
            };
            install_component_fixture(&paths, &global, component, name);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        requested,
        vec![(Component::Dxvk, None), (Component::Vkd3d, None)]
    );
    let rows = bulk_rows(&library);
    assert_eq!(library.into_ready().len(), 1);
    assert!(matches!(rows[0].status, RowStatus::Ready { .. }));
    assert_eq!(
        String::from_utf8_lossy(prompt.output())
            .matches("Download the latest release?")
            .count(),
        2
    );
}

#[test]
fn resolve_pending_with_yes_skips_without_asking() {
    let root = tempdir().unwrap();
    let mut library = waiting(vec![pending_game(
        root.path(),
        Component::Dxvk,
        "no DXVK build, run wineo download dxvk",
    )]);
    resolve_pending::<Script, Vec<u8>>(
        &paths(root.path()),
        &GlobalConfig::default(),
        &mut library,
        true,
        None,
    )
    .unwrap();
    let rows = bulk_rows(&library);
    assert!(library.into_ready().is_empty());
    assert_eq!(rows.len(), 1);
    let RowStatus::Skipped(reason) = &rows[0].status else {
        panic!("expected a skip row");
    };
    assert!(reason.contains("wineo download dxvk"), "{reason}");
}

#[test]
fn resolve_pending_interactive_decline_keeps_the_original_reason() {
    let root = tempdir().unwrap();
    let mut library = waiting(vec![pending_game(
        root.path(),
        Component::Dxvk,
        "no dxvk builds are installed",
    )]);
    let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
    resolve_pending(
        &paths(root.path()),
        &GlobalConfig::default(),
        &mut library,
        false,
        Some(&mut prompt),
    )
    .unwrap();
    let rows = bulk_rows(&library);
    assert!(library.into_ready().is_empty());
    assert_eq!(rows.len(), 1);
    assert!(
        matches!(&rows[0].status, RowStatus::Skipped(reason) if reason == "no dxvk builds are installed")
    );
}

#[test]
fn resolve_pending_asks_once_per_component_across_every_game_that_needs_it() {
    let root = tempdir().unwrap();
    let mut library = waiting(vec![
        pending_game(root.path(), Component::Dxvk, "dxvk missing"),
        pending_game(root.path(), Component::Dxvk, "dxvk missing"),
    ]);
    let mut prompt = Prompt::new(Script::new(&["n"]), Vec::new());
    resolve_pending(
        &paths(root.path()),
        &GlobalConfig::default(),
        &mut library,
        false,
        Some(&mut prompt),
    )
    .unwrap();
    assert_eq!(bulk_rows(&library).len(), 2);
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    // One question covering both games, not one each.
    assert_eq!(printed.matches("Download the latest release?").count(), 1);
    assert!(printed.contains("for 2 games"), "{printed}");
}

// -- art --

/// Runs the final review on `answers` with `art`, returning the transcript.
fn review_with_art(root: &Path, art: &mut ArtChoice, answers: &[&str]) -> String {
    let paths = paths(root);
    let global = global_with_runners(root);
    let source = heroic_source(root);
    let mut plan = plan_for(root, reviewed_import(root));
    plan.set_desktop_enabled(true);
    let mut prompt = Prompt::new(Script::new(answers), Vec::new());
    final_review(
        &ReviewContext {
            paths: &paths,
            global: &global,
            source: &source,
            verbose: false,
        },
        &mut plan,
        art,
        &mut prompt,
    )
    .unwrap();
    String::from_utf8_lossy(prompt.output()).into_owned()
}

#[test]
fn guided_review_without_a_key_notes_it_and_shows_art_none() {
    let root = tempdir().unwrap();
    let lookups = std::cell::Cell::new(0);
    let access = access(false, &lookups, Fake::default());
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    let mut art = review_art(&GlobalConfig::default(), &access, &mut prompt, "Game").unwrap();
    assert_eq!(
        String::from_utf8_lossy(prompt.output()),
        "note: run `wineo art key` to fetch art from SteamGridDB\n"
    );
    assert!(!art.enabled);

    // 9 is not a choice, since the row can't be edited without a key.
    let printed = review_with_art(root.path(), &mut art, &["9", ""]);
    assert!(printed.contains("8) desktop      yes\n"), "{printed}");
    assert!(
        printed.contains("\n     art          none\nClose Heroic and the game first.\n"),
        "{printed}"
    );
    assert!(
        printed.contains("`9` is not one of the choices"),
        "{printed}"
    );
    assert!(!art.enabled);
}

#[test]
fn guided_review_with_a_key_fetches_by_default_and_edits_the_row() {
    let root = tempdir().unwrap();
    let lookups = std::cell::Cell::new(0);
    let access = access(true, &lookups, Fake::default());
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    let mut art = review_art(&GlobalConfig::default(), &access, &mut prompt, "Game").unwrap();
    assert_eq!(prompt.output().len(), 0);
    assert!(art.enabled);

    let printed = review_with_art(root.path(), &mut art, &["9", "n", ""]);
    assert!(
        printed.contains("9) art          fetch from SteamGridDB, searching \"Game\"\n"),
        "{printed}"
    );
    assert!(
        printed.contains("Fetch art from SteamGridDB? [Y/n]: "),
        "{printed}"
    );
    assert!(printed.contains("9) art          none\n"), "{printed}");
    assert!(!art.enabled);

    let printed = review_with_art(root.path(), &mut art, &["9", "y", "Game Two", ""]);
    assert!(printed.contains("Search for [Game]: "), "{printed}");
    assert!(
        printed.contains("9) art          fetch from SteamGridDB, searching \"Game Two\"\n"),
        "{printed}"
    );
    assert!(art.enabled);
    assert_eq!(art.term, "Game Two");
    assert_eq!(
        art.log_line(),
        "  art: fetch from SteamGridDB, searching \"Game Two\"\n"
    );
}

#[test]
fn bulk_art_asks_only_with_a_key_and_follows_the_flags() {
    let lookups = std::cell::Cell::new(0);
    let ask = |found: bool, art: bool, yes: bool, answers: &[&str]| {
        let access = access(found, &lookups, Fake::default());
        let mut prompt = Prompt::new(Script::new(answers), Vec::new());
        let key = bulk_art(&access, art, yes, (!yes).then_some(&mut prompt)).unwrap();
        (
            key.is_some(),
            String::from_utf8_lossy(prompt.output()).into_owned(),
        )
    };

    let (fetch, printed) = ask(true, false, false, &[""]);
    assert!(fetch);
    assert_eq!(printed, "Fetch art for these games? [Y/n]: ");
    let (fetch, _) = ask(true, false, false, &["n"]);
    assert!(!fetch);

    let (fetch, printed) = ask(false, false, false, &[]);
    assert!(!fetch);
    assert_eq!(
        printed,
        "note: run `wineo art key` to fetch art from SteamGridDB\n"
    );

    let (fetch, printed) = ask(true, true, false, &[]);
    assert!(fetch);
    assert_eq!(printed, "");
    let (fetch, _) = ask(true, true, true, &[]);
    assert!(fetch);

    lookups.set(0);
    let (fetch, _) = ask(true, false, true, &[]);
    assert!(!fetch);
    assert_eq!(lookups.get(), 0);
}

/// A Heroic config with one sideloaded game, Alpha, and a fake wine
/// runner, classified the way `--all` does.
fn bulk_fixture(root: &Path) -> (Paths, GlobalConfig, HeroicSource, GamePlan) {
    let paths = paths(root);
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
        defaults: wineo_core::config::Defaults {
            runner_root: Some(runner_root.display().to_string()),
            prefix_root: Some(root.join("prefixes").display().to_string()),
            ..wineo_core::config::Defaults::default()
        },
        ..GlobalConfig::default()
    };
    let source = heroic_source(root);
    let listing = listing(&paths, &global, &source).unwrap();
    let library = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let mut ready = library.into_ready();
    assert_eq!(ready.len(), 1);
    let Game::Windows(game) = ready.remove(0) else {
        panic!("expected a Windows game");
    };
    (paths, global, source, game)
}

fn import_bulk_game(
    root: &Path,
    fake: Fake,
) -> (anyhow::Result<Option<String>>, String, String, Paths) {
    let (paths, global, source, game) = bulk_fixture(root);
    let lookups = std::cell::Cell::new(0);
    let access = access(true, &lookups, fake);
    let bulk = Bulk {
        paths: &paths,
        global: &global,
        access: &access,
        source: &source,
        desktop: true,
        art: Some(Key {
            value: "k".into(),
            source: wineo_core::steamgriddb::KeySource::Variable,
        }),
        verbose: false,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let cancellation = Cancellation::new().unwrap();
    let result = import_one(&bulk, &game, &cancellation, &mut out, &mut err);
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
        paths,
    )
}

#[test]
fn bulk_art_uses_the_heroic_title_before_the_desktop_entry() {
    let root = tempdir().unwrap();
    let mut fake = Fake::default();
    fake.search("Alpha", true);
    fake.icon_only();
    let (result, out, err, paths) = import_bulk_game(root.path(), fake);
    result.unwrap();
    assert!(
        out.starts_with(
            "alpha: art from SteamGridDB game 7, Hades (2020)\nicon: fetched SteamGridDB 1\n"
        ),
        "{out}"
    );
    assert_eq!(err, "");
    assert!(paths.art_dir("alpha").join("icon.png").exists());
    let entries: Vec<String> = fs::read_dir(paths.applications_dir())
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].contains("\nX-Wineo-Icon=art:"), "{}", entries[0]);
}

#[test]
fn a_failed_bulk_art_fetch_warns_without_failing_the_game() {
    let root = tempdir().unwrap();
    let (result, out, err, paths) = import_bulk_game(root.path(), Fake::default());
    result.unwrap();
    assert_eq!(out, "");
    assert!(err.contains("warning: art fetch failed:"), "{err}");
    assert!(paths.environment_config("alpha").exists());
}
