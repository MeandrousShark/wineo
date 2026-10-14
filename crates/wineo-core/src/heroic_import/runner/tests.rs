use crate::test_script::write_script;
use std::fs;
use std::path::Path;

use tempfile::tempdir;

use super::*;
use crate::config::Defaults;
use crate::heroic_import::plan::tests::{
    fake_proton_runner, fake_wine_runner, paths, reviewed_import,
};
use crate::test_snapshot::{assert_unchanged, snapshot_tree};

fn global(root: &Path) -> GlobalConfig {
    GlobalConfig {
        defaults: Defaults {
            runner_root: Some(root.join("wineo-runners").display().to_string()),
            ..Defaults::default()
        },
        ..GlobalConfig::default()
    }
}

#[test]
fn runner_copy_preflight_checks_access_and_kind() {
    let root = tempdir().unwrap();
    let mut imported = reviewed_import(root.path());
    imported.runner.source = Some(root.path().join("missing"));
    assert!(inspect_copyable_runner(&imported).is_err());
    let runner_root = root.path().join("runners");
    fake_wine_runner(&runner_root, "wine-tkg");
    imported.runner.source = Some(runner_root.join("wine-tkg"));
    imported.runner.kind = HeroicRunnerKind::Wine;
    assert!(inspect_copyable_runner(&imported).is_ok());
    imported.runner.kind = HeroicRunnerKind::Proton;
    assert!(inspect_copyable_runner(&imported).is_err());
}

#[test]
fn an_installed_runner_of_the_right_kind_is_used() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    fake_wine_runner(&root.path().join("wineo-runners"), "wine-tkg");
    let mut imported = reviewed_import(root.path());
    imported.runner.kind = HeroicRunnerKind::Wine;
    imported.runner.name = "wine-tkg".to_owned();
    let before = snapshot_tree(root.path());
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "wine-tkg"));
    assert_unchanged(&before, &snapshot_tree(root.path()));

    // An installed runner of another family isn't Heroic's choice.
    fake_proton_runner(&root.path().join("wineo-runners"), "GE-Proton9-1");
    imported.runner.kind = HeroicRunnerKind::Proton;
    imported.runner.name = "wine-tkg".to_owned();
    let before = snapshot_tree(root.path());
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(
        suggestion,
        RunnerSuggestion::Choose {
            proton: true,
            copy_blocked: None
        }
    ));
    assert_unchanged(&before, &snapshot_tree(root.path()));
}

#[test]
fn heroics_own_copy_is_offered_when_wineo_lacks_it() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    let heroic_tools = root.path().join("heroic-tools");
    fake_wine_runner(&heroic_tools, "wine-heroic");
    let mut imported = reviewed_import(root.path());
    imported.runner.kind = HeroicRunnerKind::Wine;
    imported.runner.name = "wine-heroic".to_owned();
    imported.runner.source = Some(heroic_tools.join("wine-heroic"));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    let RunnerSuggestion::Copy(copy) = suggestion else {
        panic!("expected a copy offer");
    };
    assert_eq!(copy.name, "wine-heroic");
    assert_eq!(copy.source, heroic_tools.join("wine-heroic"));

    // Naming another runner that isn't installed doesn't offer Heroic's.
    let suggestion = suggest_runner(&paths, &global, &imported, Some("nope")).unwrap();
    assert!(matches!(
        suggestion,
        RunnerSuggestion::Choose {
            proton: false,
            copy_blocked: None
        }
    ));
    // Naming Heroic's own runner does.
    let suggestion = suggest_runner(&paths, &global, &imported, Some("wine-heroic")).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

/// Heroic's runner, plus a same-named Proton runner in wineo's root with an
/// optional receipt.
fn copied_proton(root: &Path, receipt: Option<&str>) -> HeroicImport {
    let heroic_tools = root.join("heroic-tools");
    fake_proton_runner(&heroic_tools, "GE-Proton-latest");
    let installed = root.join("wineo-runners");
    fake_proton_runner(&installed, "GE-Proton-latest");
    if let Some(receipt) = receipt {
        fs::write(
            installed.join("GE-Proton-latest/.wineo-install.json"),
            receipt,
        )
        .unwrap();
    }
    let mut imported = reviewed_import(root);
    imported.runner.kind = HeroicRunnerKind::Proton;
    imported.runner.name = "GE-Proton-latest".to_owned();
    imported.runner.source = Some(heroic_tools.join("GE-Proton-latest"));
    imported
}

fn copy_receipt(source: &Path, method: &str) -> String {
    serde_json::json!({
        "architecture": "x86_64",
        "asset": "",
        "digest": "",
        "method": method,
        "provider": "ge-proton",
        "source": source,
        "stable": true,
        "variant": "baseline",
        "verified_digests": [],
        "version": "GE-Proton10-3",
    })
    .to_string()
}

#[test]
fn a_runner_copied_from_heroic_is_used() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    let source = root.path().join("heroic-tools/GE-Proton-latest");
    let imported = copied_proton(root.path(), Some(&copy_receipt(&source, "copy")));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
    let suggestion = suggest_runner(&paths, &global, &imported, Some("GE-Proton-latest")).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
}

#[test]
fn a_same_named_runner_copied_from_elsewhere_is_not_heroics() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    let other = root.path().join("elsewhere/GE-Proton-latest");
    let imported = copied_proton(root.path(), Some(&copy_receipt(&other, "copy")));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));

    // Only a copy receipt counts, not another install method.
    let source = root.path().join("heroic-tools/GE-Proton-latest");
    let imported = copied_proton(root.path(), Some(&copy_receipt(&source, "download")));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn a_same_named_runner_without_a_usable_receipt_is_not_heroics() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    let imported = copied_proton(root.path(), None);
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));

    let imported = copied_proton(root.path(), Some("not json"));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn a_copy_that_cannot_be_made_says_why() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    let mut imported = reviewed_import(root.path());
    imported.runner.kind = HeroicRunnerKind::Wine;
    imported.runner.name = "gone".to_owned();
    imported.runner.source = Some(root.path().join("missing"));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    let RunnerSuggestion::Choose {
        proton,
        copy_blocked,
    } = suggestion
    else {
        panic!("expected a choice");
    };
    assert!(!proton);
    assert!(copy_blocked.is_some());
}

#[test]
fn a_named_runner_that_is_installed_is_used_as_named() {
    let root = tempdir().unwrap();
    let global = global(root.path());
    let paths = paths(root.path());
    fake_wine_runner(&root.path().join("wineo-runners"), "wine-tkg");
    let imported = reviewed_import(root.path());
    let suggestion = suggest_runner(&paths, &global, &imported, Some("wine-tkg")).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "wine-tkg"));
}

fn scanning(root: &Path) -> GlobalConfig {
    GlobalConfig {
        external_runners: Some(true),
        ..global(root)
    }
}

/// A game whose Proton runner is `name` in Heroic's own tools folder under
/// the home at `root`, where wineo's scan finds it.
fn heroic_game(root: &Path, name: &str) -> HeroicImport {
    let heroic = root.join(".config/heroic");
    fs::create_dir_all(&heroic).unwrap();
    fs::write(heroic.join("config.json"), "{}").unwrap();
    let tools = heroic.join("tools/proton");
    fake_proton_runner(&tools, name);
    let mut imported = reviewed_import(&root.join("prefix"));
    imported.runner.kind = HeroicRunnerKind::Proton;
    imported.runner.name = name.to_owned();
    imported.runner.source = Some(tools.join(name));
    imported
}

#[test]
fn heroics_build_is_used_in_place_when_scanning_is_on() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let imported = heroic_game(root.path(), "GE-Proton-latest");
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
    let suggestion = suggest_runner(&paths, &global, &imported, Some("GE-Proton-latest")).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
    // Nothing is copied into wineo's root.
    assert!(!root.path().join("wineo-runners").exists());
}

#[test]
fn heroics_build_is_still_copied_when_scanning_is_off() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let imported = heroic_game(root.path(), "GE-Proton-latest");
    let unset = global(root.path());
    let off = GlobalConfig {
        external_runners: Some(false),
        ..global(root.path())
    };
    for global in [unset, off] {
        let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
        let suggestion =
            suggest_runner(&paths, &global, &imported, Some("GE-Proton-latest")).unwrap();
        assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
    }
}

#[test]
fn heroics_build_is_still_copied_in_a_portable_folder() {
    let root = tempdir().unwrap();
    let home = root.path().as_os_str().to_owned();
    let portable = root.path().join("portable");
    let paths =
        Paths::select(|key| (key == "HOME").then(|| home.clone()), Some(&portable)).unwrap();
    let global = scanning(root.path());
    let imported = heroic_game(root.path(), "GE-Proton-latest");
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn a_same_named_build_in_wineos_root_wins_over_heroics() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let imported = heroic_game(root.path(), "GE-Proton-latest");
    let wineo = root.path().join("wineo-runners");
    fake_proton_runner(&wineo, "GE-Proton-latest");
    fs::write(wineo.join("GE-Proton-latest/version"), "2 GE-Proton11-7\n").unwrap();
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));
    let resolved = Runners::new(&paths, &global)
        .unwrap()
        .resolve("GE-Proton-latest")
        .unwrap();
    assert_eq!(resolved.source(), crate::external_runners::Source::Wineo);
    assert_eq!(fs::read_dir(&wineo).unwrap().count(), 1);
}

#[test]
fn a_folder_name_wineo_cannot_use_is_known_by_its_derived_name() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let imported = heroic_game(root.path(), "Proton 9.0");
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "Proton-9.0"));
    let suggestion = suggest_runner(&paths, &global, &imported, Some("Proton 9.0")).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "Proton-9.0"));
}

#[test]
fn a_build_outside_the_scanned_folders_is_still_copied() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let mut imported = heroic_game(root.path(), "GE-Proton-latest");
    let elsewhere = root.path().join("custom-heroic-tools");
    fake_proton_runner(&elsewhere, "GE-Proton-latest");
    imported.runner.source = Some(elsewhere.join("GE-Proton-latest"));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn heroics_build_of_the_wrong_kind_is_not_used_in_place() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let mut imported = heroic_game(root.path(), "GE-Proton-latest");
    imported.runner.kind = HeroicRunnerKind::Wine;
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(!matches!(suggestion, RunnerSuggestion::Use(_)));
}

#[test]
fn an_alias_named_folder_is_never_used_in_place() {
    // Each folder is called an alias name, or derives to one, and wineo
    // has a working target for that alias in its own root.
    for (folder, target) in [
        ("GE-Proton-Latest", "GE-Proton11-7"),
        ("GE Proton Latest", "GE-Proton11-7"),
        (
            "Proton-CachyOS-Latest",
            "proton-cachyos-11.0-20260703-slr-x86_64",
        ),
        (
            "Proton CachyOS Latest",
            "proton-cachyos-11.0-20260703-slr-x86_64",
        ),
    ] {
        let root = tempdir().unwrap();
        let global = scanning(root.path());
        let paths = paths(root.path());
        let imported = heroic_game(root.path(), folder);
        fake_proton_runner(&root.path().join("wineo-runners"), target);
        // Built after the folders exist, so it sees them.
        let runners = Runners::new(&paths, &global).unwrap();
        let alias = crate::external_runners::derived_name(folder);
        assert!(crate::runner_alias::is_alias(&alias), "{folder}");
        assert!(runners.resolve(&alias).is_ok(), "{folder}: alias target");
        let source = imported.runner.source.clone().unwrap();
        assert!(runners.external_names_at(&source).is_empty(), "{folder}");
        // Naming the alias outright is the person's own choice, so only
        // Heroic's name for it is checked.
        for requested in [None, Some(folder)] {
            let suggestion = suggest_runner(&paths, &global, &imported, requested).unwrap();
            assert!(
                !matches!(suggestion, RunnerSuggestion::Use(_)),
                "{folder} {requested:?}: {suggestion:?}"
            );
        }
    }
}

#[test]
fn a_wine_named_folder_is_never_used_in_place() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    // `wine` would resolve to the system wine.
    let dir = root.path().join(".config/heroic/tools/wine/wine");
    fs::create_dir_all(dir.join("bin")).unwrap();
    fs::write(root.path().join(".config/heroic/config.json"), "{}").unwrap();
    for executable in ["wine", "wineserver"] {
        let path = dir.join("bin").join(executable);
        write_script(&path, "#!/bin/sh\n");
    }
    let mut imported = reviewed_import(&root.path().join("prefix"));
    imported.runner.kind = HeroicRunnerKind::Wine;
    imported.runner.name = "wine".to_owned();
    imported.runner.source = Some(dir.clone());
    let runners = Runners::new(&paths, &global).unwrap();
    // The folder is really among the scanned ones.
    assert!(
        crate::external_runners::folders(&paths)
            .iter()
            .any(|folder| dir.starts_with(&folder.path))
    );
    assert!(runners.external_names_at(&dir).is_empty());
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn a_build_reached_through_a_symlink_is_still_heroics() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let mut imported = heroic_game(root.path(), "GE-Proton-latest");

    // The game's runner path goes through a link to Heroic's folder.
    let link = root.path().join("link");
    symlink(imported.runner.source.as_ref().unwrap(), &link).unwrap();
    imported.runner.source = Some(link);
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "GE-Proton-latest"));

    // Heroic's folder is itself a link into another launcher's tree.
    let real = root.path().join("elsewhere");
    fake_proton_runner(&real, "GE-Proton11-1");
    let tools = root.path().join(".config/heroic/tools/proton");
    symlink(real.join("GE-Proton11-1"), tools.join("Linked")).unwrap();
    imported.runner.source = Some(real.join("GE-Proton11-1"));
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Use(name) if name == "Linked"));
}

#[test]
fn a_broken_same_named_build_in_wineos_root_is_not_used() {
    let root = tempdir().unwrap();
    let global = scanning(root.path());
    let paths = paths(root.path());
    let imported = heroic_game(root.path(), "GE-Proton-latest");
    // The folder owns the name even though nothing in it works.
    fs::create_dir_all(root.path().join("wineo-runners/GE-Proton-latest")).unwrap();
    let suggestion = suggest_runner(&paths, &global, &imported, None).unwrap();
    assert!(matches!(suggestion, RunnerSuggestion::Copy(_)));
}

#[test]
fn a_scanning_import_registers_heroics_name_and_copies_nothing() {
    use crate::cancel::Token;
    use crate::heroic::HeroicSource;
    use crate::heroic_import::{Options, apply, plan_game};

    let root = tempdir().unwrap();
    let home = root.path();
    let prefix = home.join("heroic-prefix");
    fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    let exe = prefix.join("drive_c/Games/Game.exe");
    fs::write(&exe, "game executable").unwrap();
    let tools = home.join(".config/heroic/tools/wine");
    let runner = fake_wine_runner(&tools, "wine-heroic");
    let bin = runner.wine_path().unwrap().display().to_string();
    let config = home.join(".config/heroic");
    fs::create_dir_all(config.join("sideload_apps")).unwrap();
    fs::create_dir_all(config.join("GamesConfig")).unwrap();
    fs::write(
            config.join("config.json"),
            format!(
                r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"wine-heroic","bin":"{bin}"}}}}}}"#,
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

    let mut global = scanning(home);
    global.defaults.prefix_root = Some(home.join("prefixes").display().to_string());
    let paths = paths(home);
    let source = HeroicSource::from_config_dir_with_home(config, home.to_owned());
    let imported = source.inspect("a").unwrap();
    let RunnerSuggestion::Use(name) = suggest_runner(&paths, &global, &imported, None).unwrap()
    else {
        panic!("expected Heroic's build to be used in place");
    };
    assert_eq!(name, "wine-heroic");

    let plan = plan_game(&paths, &global, &source, "a", &name, Options::default()).unwrap();
    let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(registered.runner().unwrap().name(), "wine-heroic");
    let written = fs::read_to_string(paths.environment_config("alpha")).unwrap();
    assert!(written.contains("runner = \"wine-heroic\""), "{written}");
    // The build stays in Heroic's folder, and nothing lands in wineo's.
    assert!(
        Runners::new(&paths, &global)
            .unwrap()
            .resolve("wine-heroic")
            .unwrap()
            .wine_path()
            .unwrap()
            .starts_with(&tools)
    );
    let copied = fs::read_dir(home.join("wineo-runners")).map_or(0, Iterator::count);
    assert_eq!(copied, 0);
}
