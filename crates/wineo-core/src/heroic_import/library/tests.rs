use crate::test_script::write_script;
use std::fs;

use tempfile::tempdir;

use super::*;
use crate::config::Defaults;
use crate::heroic_import::plan::tests::{fake_proton_runner, fake_wine_runner, paths};
use crate::test_snapshot::{assert_unchanged, snapshot_tree};

fn candidate(kind: HeroicGameKind, issues: Vec<HeroicIssue>) -> HeroicCandidate {
    HeroicCandidate {
        id: "game".to_owned(),
        title: "Game".to_owned(),
        kind,
        prefix: None,
        runner: None,
        executable: None,
        poster: None,
        issues,
    }
}

fn issue(kind: HeroicIssueKind, field: &str, message: &str) -> HeroicIssue {
    HeroicIssue {
        kind,
        field: Some(field.to_owned()),
        message: message.to_owned(),
    }
}

#[test]
fn blocker_allows_a_clean_windows_candidate() {
    assert!(blocker(&candidate(HeroicGameKind::Windows, Vec::new())).is_none());
}

#[test]
fn blocker_allows_invalid_launcher_args() {
    let candidate = candidate(
        HeroicGameKind::Windows,
        vec![issue(HeroicIssueKind::Invalid, "launcherArgs", "bad args")],
    );
    assert!(blocker(&candidate).is_none());
}

#[test]
fn blocker_rejects_non_windows_kinds() {
    let candidate = candidate(
        HeroicGameKind::Browser,
        vec![issue(
            HeroicIssueKind::Unsupported,
            "platform",
            "native games are not Windows prefix imports",
        )],
    );
    let issue = blocker(&candidate).unwrap();
    assert_eq!(issue.message, "native games are not Windows prefix imports");
}

#[test]
fn blocker_rejects_missing_settings() {
    let candidate = candidate(
        HeroicGameKind::Windows,
        vec![issue(HeroicIssueKind::Missing, "exe", "no executable set")],
    );
    assert_eq!(blocker(&candidate).unwrap().message, "no executable set");
}

#[test]
fn blocker_rejects_unsupported_runner() {
    let candidate = candidate(
        HeroicGameKind::Windows,
        vec![issue(
            HeroicIssueKind::Unsupported,
            "runner",
            "only manually added sideloaded games are supported",
        )],
    );
    assert!(blocker(&candidate).is_some());
}

#[test]
fn a_native_candidate_is_blocked_by_its_own_rule() {
    let clean = candidate(HeroicGameKind::Native, Vec::new());
    assert!(blocker(&clean).is_none());
    // Unsupported settings like wrappers are only reported for a native game.
    let wrapped = candidate(
        HeroicGameKind::Native,
        vec![issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "wrappers need manual setup",
        )],
    );
    assert!(blocker(&wrapped).is_none());
    let missing = candidate(
        HeroicGameKind::Native,
        vec![issue(
            HeroicIssueKind::Missing,
            "executable",
            "executable not found",
        )],
    );
    assert_eq!(blocker(&missing).unwrap().message, "executable not found");
}

fn existing(
    name: &str,
    heroic_id: Option<&str>,
    prefix: &Path,
    target_exes: &[&Path],
) -> ExistingEnvironment {
    ExistingEnvironment {
        name: name.to_owned(),
        heroic_id: heroic_id.map(str::to_owned),
        snapshot: None,
        prefix: Some(prefix.to_owned()),
        target_exes: target_exes.iter().map(|exe| exe.to_path_buf()).collect(),
    }
}

#[test]
fn match_existing_matches_by_recorded_id_first() {
    let existing = vec![existing(
        "palworld",
        Some("abc"),
        Path::new("/prefixes/palworld"),
        &[],
    )];
    let matched = match_existing(&existing, "abc", None, None, "something-else").unwrap();
    assert_eq!(matched.name, "palworld");
    assert!(!matched.needs_id);
}

#[test]
fn match_existing_does_not_claim_another_id_through_legacy_fallbacks() {
    let prefix = Path::new("/prefixes/shared");
    let exe = Path::new("/games/shared.exe");
    let existing = vec![
        existing("shared", Some("game-a"), prefix, &[exe]),
        existing("legacy", None, prefix, &[exe]),
    ];
    let matched = match_existing(&existing, "game-b", Some(prefix), Some(exe), "shared").unwrap();
    assert_eq!(matched.name, "legacy");
    assert!(matched.needs_id);

    let recorded_only = &existing[..1];
    assert!(match_existing(recorded_only, "game-b", Some(prefix), Some(exe), "shared").is_none());
    let matched =
        match_existing(recorded_only, "game-a", Some(prefix), Some(exe), "other").unwrap();
    assert_eq!(matched.name, "shared");
    assert!(!matched.needs_id);
}

#[test]
fn a_stale_backfill_is_rejected_without_changing_the_config() {
    for change in ["config", "id", "removed"] {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let path = paths.environment_config("legacy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let prefix = root.path().join("shared-prefix");
        let original = format!(
            "# keep this\nprefix = {:?}\nrunner = \"wine\"\narch = \"win64\"\n",
            prefix.to_string_lossy()
        );
        fs::write(&path, &original).unwrap();
        let mut existing = existing_environments(&paths).unwrap();
        let matched = match_existing(&existing, "candidate", Some(&prefix), None, "other").unwrap();
        assert!(matched.needs_id);
        let mut backfills = Vec::new();
        queue_backfill(&mut existing, &mut backfills, &matched, "candidate");
        assert_eq!(backfills.len(), 1);

        let changed = match change {
            "config" => original.replace("wine", "other"),
            "id" => format!("{original}heroic_id = \"different\"\n"),
            _ => String::new(),
        };
        if change == "removed" {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, &changed).unwrap();
        }
        let error = backfills[0].record(&paths).unwrap_err();
        assert!(
            error.to_string().contains("retry the import")
                || error.to_string().contains("does not exist"),
            "{error:#}"
        );
        if change == "removed" {
            assert!(!path.exists());
        } else {
            assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        }
    }
}

#[test]
fn match_existing_falls_back_to_the_adopted_prefix() {
    let existing = vec![existing(
        "zomboid",
        None,
        Path::new("/prefixes/zomboid"),
        &[],
    )];
    let matched = match_existing(
        &existing,
        "different-id",
        Some(Path::new("/prefixes/zomboid")),
        None,
        "not-the-name",
    )
    .unwrap();
    assert_eq!(matched.name, "zomboid");
    assert!(matched.needs_id, "a legacy match should be backfilled");
}

#[test]
fn match_existing_falls_back_to_a_target_running_the_same_exe() {
    let existing = vec![existing(
        "Zomboid",
        None,
        Path::new("/prefixes/other"),
        &[Path::new(
            "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
        )],
    )];
    let matched = match_existing(
        &existing,
        "5HYciurSM5YdUGzasx38E7",
        Some(Path::new("/elsewhere")),
        Some(Path::new(
            "/home/me/Games/ProjectZomboid/ProjectZomboid64.exe",
        )),
        "project-zomboid",
    )
    .unwrap();
    assert_eq!(matched.name, "Zomboid");
    assert!(matched.needs_id, "a legacy match should be backfilled");
}

#[test]
fn match_existing_falls_back_to_the_default_name() {
    let existing = vec![existing("hades", None, Path::new("/prefixes/other"), &[])];
    let matched = match_existing(
        &existing,
        "different-id",
        Some(Path::new("/elsewhere")),
        None,
        "hades",
    )
    .unwrap();
    assert_eq!(matched.name, "hades");
    assert!(matched.needs_id);
}

#[test]
fn match_existing_returns_none_when_nothing_matches() {
    let existing = vec![existing("hades", None, Path::new("/prefixes/hades"), &[])];
    assert!(
        match_existing(
            &existing,
            "id",
            Some(Path::new("/elsewhere")),
            Some(Path::new("/elsewhere/Game.exe")),
            "not-hades"
        )
        .is_none()
    );
}

#[test]
fn a_queued_backfill_claims_the_environment_for_later_candidates() {
    let prefix = Path::new("/prefixes/shared");
    let mut existing = vec![ExistingEnvironment {
        snapshot: Some(dummy_snapshot()),
        ..existing("legacy", None, prefix, &[])
    }];
    let mut backfills = Vec::new();
    let first = match_existing(&existing, "one", Some(prefix), None, "x").unwrap();
    queue_backfill(&mut existing, &mut backfills, &first, "one");
    assert!(match_existing(&existing, "two", Some(prefix), None, "x").is_none());
    let again = match_existing(&existing, "one", Some(prefix), None, "x").unwrap();
    assert!(!again.needs_id);
    assert_eq!(backfills.len(), 1);
}

/// A snapshot of some environment config, since there's no other way to
/// make one.
fn dummy_snapshot() -> HeroicIdSnapshot {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let path = paths.environment_config("dummy");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "prefix = \"/x\"\nrunner = \"wine\"\narch = \"win64\"\n",
    )
    .unwrap();
    Environment::load_for_heroic_match(&paths, "dummy")
        .unwrap()
        .1
}

// -- planning a library --

/// A Heroic library under `root/heroic` and the runners wineo needs.
struct Library {
    root: PathBuf,
    games: Vec<String>,
}

impl Library {
    fn new(root: &Path) -> Self {
        fs::create_dir_all(root.join("heroic/GamesConfig")).unwrap();
        fs::create_dir_all(root.join("heroic/sideload_apps")).unwrap();
        fs::write(
            root.join("heroic/config.json"),
            r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
        )
        .unwrap();
        fake_wine_runner(&root.join("runners"), "fake");
        fake_proton_runner(&root.join("runners"), "GE-Proton9-1");
        Self {
            root: root.to_owned(),
            games: Vec::new(),
        }
    }

    fn prefix(&self, name: &str) -> PathBuf {
        let prefix = self.root.join("prefixes").join(name);
        fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        prefix
    }

    /// A Windows sideload with its own prefix and the given extra settings.
    fn windows(&mut self, id: &str, title: &str, prefix: &str, settings: &str) {
        let prefix = self.prefix(prefix);
        let exe = prefix.join(format!("drive_c/Games/{id}.exe"));
        fs::write(&exe, "game").unwrap();
        fs::write(
            self.root.join(format!("heroic/GamesConfig/{id}.json")),
            format!(
                r#"{{"{id}":{{"winePrefix":"{}"{}{settings}}}}}"#,
                prefix.display(),
                if settings.is_empty() { "" } else { "," }
            ),
        )
        .unwrap();
        self.games.push(format!(
                r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}"#,
                exe.display()
            ));
    }

    fn native(&mut self, id: &str, title: &str) {
        let exe = self.root.join(format!("games/{id}"));
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\n");
        self.games.push(format!(
                r#"{{"app_name":"{id}","title":"{title}","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}"#,
                exe.display()
            ));
    }

    fn write(&self) -> (Paths, GlobalConfig, HeroicListing) {
        fs::write(
            self.root.join("heroic/sideload_apps/library.json"),
            format!(r#"{{"games":[{}]}}"#, self.games.join(",")),
        )
        .unwrap();
        let paths = paths(&self.root);
        let global = GlobalConfig {
            defaults: Defaults {
                runner_root: Some(self.root.join("runners").display().to_string()),
                prefix_root: Some(self.root.join("wineo-prefixes").display().to_string()),
                ..Defaults::default()
            },
            ..GlobalConfig::default()
        };
        let source =
            HeroicSource::from_config_dir_with_home(self.root.join("heroic"), self.root.clone());
        let listing = listing(&paths, &global, &source).unwrap();
        (paths, global, listing)
    }
}

fn names(plan: &LibraryPlan) -> Vec<String> {
    plan.entries()
        .iter()
        .map(|entry| match entry {
            Entry::Ready(game) => format!("ready {}", game.name()),
            Entry::Pending(game) => format!("pending {}", game.plan().name()),
            Entry::Skipped(skipped) => format!("skipped {}", skipped.name),
        })
        .collect()
}

fn install_dxvk(paths: &Paths, global: &GlobalConfig) {
    let root = crate::components::Builds::new(paths, global, Component::Dxvk)
        .unwrap()
        .root()
        .to_owned();
    for arch in ["x64", "x32"] {
        let dir = root.join("dxvk-2.7").join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("dxgi.dll"), b"fixture").unwrap();
    }
}

fn install_vkd3d(paths: &Paths, global: &GlobalConfig) {
    let root = crate::components::Builds::new(paths, global, Component::Vkd3d)
        .unwrap()
        .root()
        .to_owned();
    for arch in ["x64", "x86"] {
        let dir = root.join("vkd3d-proton-2.14").join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("d3d12.dll"), b"fixture").unwrap();
    }
}

#[test]
fn a_library_is_sorted_into_ready_pending_skipped_and_imported() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "Bravo", "p2", r#""autoInstallDxvk":true"#);
    library.windows("c", "Charlie", "p3", r#""launcherArgs":"-a \"unbalanced""#);
    library.windows("d", "Delta", "p1", "");
    library.native("e", "Echo");
    library.windows("f", "Fox", "p4", r#""wrapperOptions":[{"exe":"x"}]"#);
    let (paths, global, listing) = library.write();
    // An environment imported before Heroic ids were recorded.
    let legacy = paths.environment_config("fox");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
        &legacy,
        "prefix = \"/somewhere\"\nrunner = \"wine\"\narch = \"win64\"\n",
    )
    .unwrap();
    let before = fs::read_to_string(&legacy).unwrap();

    let tree_before = snapshot_tree(root.path());
    let explicit = listing.source.config_dir().to_string_lossy().into_owned();
    let sources = super::sources(&paths, Some(&explicit)).unwrap();
    assert_eq!(sources.len(), 1);
    let refreshed = super::listing(&paths, &global, &sources[0]).unwrap();

    let plan = plan_library(
        &paths,
        &global,
        &refreshed,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let selected = BTreeSet::from(["f".to_owned()]);
    let selected_plan = plan_selection(
        &paths,
        &global,
        &refreshed,
        &selected,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(selected_plan.already(), ["fox"]);
    assert_unchanged(&tree_before, &snapshot_tree(root.path()));
    assert_eq!(plan.runner(), Some("fake"));
    assert_eq!(
        names(&plan),
        [
            "ready echo",
            "ready alpha",
            "pending bravo",
            "skipped charlie",
            "ready delta",
        ]
    );
    assert_eq!(plan.already(), ["fox"]);
    let Entry::Skipped(charlie) = &plan.entries()[3] else {
        panic!("expected a skip");
    };
    assert_eq!(charlie.reason, SkipReason::LauncherArgs);
    assert_eq!(charlie.title, "Charlie");

    // Planning read the legacy environment but wrote nothing.
    assert_eq!(fs::read_to_string(&legacy).unwrap(), before);
    assert_eq!(plan.backfills().len(), 1);
    assert_eq!(plan.backfills()[0].environment, "fox");
    assert_eq!(plan.backfills()[0].heroic_id, "f");
    plan.record_backfills(&paths).unwrap();
    let after = fs::read_to_string(&legacy).unwrap();
    assert!(after.contains("heroic_id = \"f\""), "{after}");
}

#[test]
fn shared_prefixes_are_warned_about_on_each_game_that_uses_one() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("d", "Delta", "p1", "");
    let (paths, global, listing) = library.write();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let Entry::Ready(Game::Windows(alpha)) = &plan.entries()[0] else {
        panic!("expected a ready game");
    };
    assert_eq!(
        alpha.warnings(),
        vec![Warning::SharedPrefix(vec![HeroicSharedPrefix {
            id: "d".to_owned(),
            title: "Delta".to_owned()
        }])]
    );
}

#[test]
fn a_selection_plans_only_the_chosen_games() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "Bravo", "p2", "");
    library.windows("d", "Delta", "p1", "");
    library.native("e", "Echo");
    let (paths, global, listing) = library.write();
    let selected = BTreeSet::from(["a".to_owned(), "e".to_owned()]);
    let plan = plan_selection(
        &paths,
        &global,
        &listing,
        &selected,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(names(&plan), ["ready echo", "ready alpha"]);
    // Delta isn't imported, but it still uses Alpha's prefix.
    let Entry::Ready(Game::Windows(alpha)) = &plan.entries()[1] else {
        panic!("expected a ready game");
    };
    assert_eq!(
        alpha.warnings(),
        vec![Warning::SharedPrefix(vec![HeroicSharedPrefix {
            id: "d".to_owned(),
            title: "Delta".to_owned()
        }])]
    );
    let natives = BTreeSet::from(["e".to_owned()]);
    let plan = plan_selection(
        &paths,
        &global,
        &listing,
        &natives,
        None,
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(names(&plan), ["ready echo"]);
}

#[test]
fn a_proton_game_needs_a_proton_runner_and_the_other_way_around() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    let (paths, global, listing) = library.write();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("GE-Proton9-1"),
        &LibraryOptions::default(),
    );
    // The Wine game can't be imported with a Proton runner, and the
    // Proton prefix layout would be needed besides.
    let plan = plan.unwrap();
    let Entry::Skipped(skipped) = &plan.entries()[0] else {
        panic!("expected a skip");
    };
    assert_eq!(skipped.reason, SkipReason::NeedsRunner { proton: false });
}

#[test]
fn a_native_only_library_needs_no_runner() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.native("e", "Echo");
    let (paths, global, listing) = library.write();
    let plan = plan_library(&paths, &global, &listing, None, &LibraryOptions::default()).unwrap();
    assert_eq!(plan.runner(), None);
    assert_eq!(names(&plan), ["ready echo"]);
    let ready = plan.into_ready();
    assert!(matches!(&ready[0], Game::Native(native) if native.name() == "echo"));

    library.windows("a", "Alpha", "p1", "");
    let (paths, global, listing) = library.write();
    assert!(plan_library(&paths, &global, &listing, None, &LibraryOptions::default()).is_err());
}

#[test]
fn names_taken_by_environments_or_earlier_games_are_skipped() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "alpha!", "p2", "");
    library.native("e", "Zulu");
    library.windows("z", "Zulu", "p3", "");
    let (paths, global, listing) = library.write();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let skipped: Vec<_> = plan
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Skipped(skipped) => Some(skipped),
            _ => None,
        })
        .collect();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0].reason, SkipReason::NameTaken("alpha".to_owned()));
    assert_eq!(skipped[1].reason, SkipReason::NameTaken("zulu".to_owned()));
}

#[test]
fn pending_builds_are_grouped_rechecked_and_settled() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "Bravo", "p2", r#""autoInstallDxvk":true"#);
    library.windows("c", "Charlie", "p3", r#""autoInstallDxvk":true"#);
    library.native("e", "Echo");
    let (paths, global, listing) = library.write();
    let tree_before = snapshot_tree(root.path());
    let refreshed = super::listing(&paths, &global, &listing.source).unwrap();
    let mut plan = plan_library(
        &paths,
        &global,
        &refreshed,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_unchanged(&tree_before, &snapshot_tree(root.path()));
    assert_eq!(
        plan.pending_builds(),
        vec![BuildOffer {
            component: Component::Dxvk,
            versions: vec![None],
            games: 2
        }]
    );
    // Nothing changed, so rechecking leaves them waiting, with the reason.
    plan.recheck(&paths, &global);
    assert_eq!(plan.pending_builds().len(), 1);

    // Settling without the build skips them, at the end.
    let mut without = plan.clone();
    without.settle(&paths, &global);
    assert_eq!(
        names(&without),
        [
            "ready echo",
            "ready alpha",
            "skipped bravo",
            "skipped charlie"
        ]
    );
    let Entry::Skipped(skipped) = &without.entries()[2] else {
        panic!("expected a skip");
    };
    let SkipReason::Unavailable(reason) = &skipped.reason else {
        panic!("expected an unavailable build");
    };
    assert!(reason.contains("wineo download dxvk"), "{reason}");

    // With the build, the same games are ready, after the ones that never waited.
    install_dxvk(&paths, &global);
    plan.recheck(&paths, &global);
    plan.settle(&paths, &global);
    assert_eq!(
        names(&plan),
        ["ready echo", "ready alpha", "ready bravo", "ready charlie"]
    );
    assert!(plan.pending_builds().is_empty());
    let ready: Vec<String> = plan
        .into_ready()
        .iter()
        .map(|game| game.name().to_owned())
        .collect();
    // Native games are imported last.
    assert_eq!(ready, ["alpha", "bravo", "charlie", "echo"]);
}

#[test]
fn pending_builds_list_each_pinned_version_once() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    let (paths, global, listing) = library.write();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let Entry::Ready(Game::Windows(alpha)) = plan.entries()[0].clone() else {
        panic!("expected a ready game");
    };
    let waiting = |version: Option<&str>| {
        Entry::Pending(PendingGame::new(
            alpha.clone(),
            Component::Vkd3d,
            version.map(str::to_owned),
            "missing".to_owned(),
        ))
    };
    let plan = LibraryPlan::from_entries(
        Some("fake".to_owned()),
        Vec::new(),
        vec![
            waiting(Some("v2")),
            waiting(None),
            waiting(Some("v2")),
            waiting(Some("v3")),
        ],
    );
    assert_eq!(
        plan.pending_builds(),
        vec![BuildOffer {
            component: Component::Vkd3d,
            versions: vec![Some("v2".to_owned()), None, Some("v3".to_owned())],
            games: 4
        }]
    );
}

#[test]
fn a_broken_prefix_root_fails_the_plan_instead_of_skipping_each_game() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.native("e", "Echo");
    let (paths, mut global, listing) = library.write();
    global.defaults.prefix_root = Some(String::new());

    let error = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("empty path"),
        "unexpected error: {error:#}"
    );

    // Adopting never reads it.
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions {
            adopt: true,
            ..LibraryOptions::default()
        },
    )
    .unwrap();
    assert_eq!(plan.ready_count(), 2);
}

#[test]
fn adopting_plans_every_game_in_place() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    let (paths, global, listing) = library.write();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions {
            adopt: true,
            ..LibraryOptions::default()
        },
    )
    .unwrap();
    let Entry::Ready(Game::Windows(alpha)) = &plan.entries()[0] else {
        panic!("expected a ready game");
    };
    assert!(alpha.is_adopt());
    assert_eq!(alpha.request().destination, None);
}

#[test]
fn a_listing_marks_a_native_game_whose_exe_cannot_run() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.native("e", "Echo");
    fs::remove_file(root.path().join("games/e")).unwrap();
    let (_, _, listing) = library.write();
    let issues = &listing.candidates[0].issues;
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == HeroicIssueKind::Missing
                && issue.field.as_deref() == Some("executable")
                && issue.message.starts_with("executable not found: ")),
        "{issues:?}"
    );
    assert!(blocker(&listing.candidates[0]).is_some());
}

#[test]
fn sources_uses_an_explicit_directory_as_given() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let sources = sources(&paths, Some("~/heroic")).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].config_dir(), root.path().join("heroic"));
}

#[test]
fn planning_a_game_and_checking_installed_builds_does_not_write_or_lock() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows(
        "a",
        "Alpha",
        "p1",
        r#""autoInstallDxvk":true,"autoInstallVkd3d":true"#,
    );
    let (paths, global, listing) = library.write();
    install_dxvk(&paths, &global);
    install_vkd3d(&paths, &global);
    let tree_before = snapshot_tree(root.path());

    let plan = crate::heroic_import::plan_game(
        &paths,
        &global,
        &listing.source,
        "a",
        "fake",
        crate::heroic_import::Options::default(),
    )
    .unwrap();
    plan.check_builds(&paths, &global).unwrap();
    let library = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(library.ready_count(), 1);

    assert_unchanged(&tree_before, &snapshot_tree(root.path()));
}

fn parses(file: &str) -> usize {
    crate::heroic::PARSES.with_borrow(|parses| parses.get(file).copied().unwrap_or(0))
}

fn clear_parses() {
    crate::heroic::PARSES.with_borrow_mut(|parses| parses.clear());
}

#[test]
fn a_bulk_plan_parses_heroics_library_and_settings_once() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    for index in 0..20 {
        library.windows(
            &format!("g{index:02}"),
            &format!("Game {index:02}"),
            &format!("p{index:02}"),
            "",
        );
    }
    library.native("n", "Native");
    let (paths, global, listing) = library.write();

    clear_parses();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.ready_count(), 21);
    assert_eq!(parses("library.json"), 1);
    assert_eq!(parses("config.json"), 1);
}

#[test]
fn planning_one_game_parses_heroics_library_once() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "Beta", "p1", "");
    library.windows("c", "Gamma", "p2", "");
    let (paths, global, listing) = library.write();

    clear_parses();
    let plan = crate::heroic_import::plan_game(
        &paths,
        &global,
        &listing.source,
        "a",
        "fake",
        crate::heroic_import::Options::default(),
    )
    .unwrap();
    assert_eq!(parses("library.json"), 1);
    assert_eq!(parses("config.json"), 1);
    assert_eq!(
        plan.warnings(),
        vec![Warning::SharedPrefix(vec![HeroicSharedPrefix {
            id: "b".to_owned(),
            title: "Beta".to_owned()
        }])]
    );
}

#[test]
fn a_library_that_is_all_imported_already_is_not_read_to_plan() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("f", "Fox", "p1", "");
    let (paths, global, listing) = library.write();
    let legacy = paths.environment_config("fox");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
        &legacy,
        "prefix = \"/somewhere\"\nrunner = \"wine\"\narch = \"win64\"\n",
    )
    .unwrap();

    clear_parses();
    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.already(), ["fox"]);
    assert_eq!(parses("library.json"), 0);
}

#[test]
fn a_game_gone_from_the_library_is_reported_before_a_broken_global_config() {
    let root = tempdir().unwrap();
    let mut library = Library::new(root.path());
    library.windows("a", "Alpha", "p1", "");
    library.windows("b", "Bravo", "p2", "");
    library.native("n", "Native");
    let (paths, global, listing) = library.write();
    // Heroic drops two games and breaks its config after the listing.
    let kept: Vec<&String> = library
        .games
        .iter()
        .filter(|g| g.contains("\"b\""))
        .collect();
    let kept = kept
        .iter()
        .map(|g| g.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let heroic = root.path().join("heroic");
    fs::write(
        heroic.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{kept}]}}"#),
    )
    .unwrap();
    fs::write(heroic.join("config.json"), "{").unwrap();

    let plan = plan_library(
        &paths,
        &global,
        &listing,
        Some("fake"),
        &LibraryOptions::default(),
    )
    .unwrap();
    let reasons: Vec<String> = plan
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Skipped(skipped) => match &skipped.reason {
                SkipReason::Unreadable(message) => message.clone(),
                other => panic!("{other:?}"),
            },
            _ => panic!("expected a skip"),
        })
        .collect();
    assert_eq!(reasons.len(), 3);
    assert!(
        reasons[0].contains("not in sideload library"),
        "{reasons:?}"
    );
    assert!(
        reasons[1].contains("not in sideload library"),
        "{reasons:?}"
    );
    assert!(reasons[2].contains("invalid Heroic JSON"), "{reasons:?}");
}
