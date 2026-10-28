use std::ffi::OsString;
use std::os::unix::fs::symlink;

use super::super::*;
use crate::environment::Environment;
use crate::validate::{self, Code, Fix};

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        Self { _root: root, paths }
    }

    fn native(&self, name: &str) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "kind = 'native'\n").unwrap();
    }

    fn wine(&self, name: &str) -> PathBuf {
        let prefix = self.paths.default_prefix_root().join(name);
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "prefix = '{}'\nrunner = 'wine'\narch = 'win64'\n",
                prefix.display()
            ),
        )
        .unwrap();
        prefix
    }

    /// Trashes `name` and gives its manifest a version this wineo doesn't
    /// know, so it parses but fails verification.
    fn unsupported(&self, name: &str, include_prefix: bool) -> Entry {
        let entry = remove(&self.paths, name, include_prefix).unwrap();
        let mut manifest = read_manifest(&self.paths, &entry.id).unwrap();
        manifest.version = VERSION + 1;
        write_manifest(&self.paths, &manifest).unwrap();
        entry
    }

    fn garbled(&self, name: &str) -> Entry {
        let entry = remove(&self.paths, name, false).unwrap();
        let file = entry_dir(&self.paths, &entry.id).unwrap().join(MANIFEST);
        fs::write(file, "this is not a manifest").unwrap();
        entry
    }

    fn dir(&self, entry: &Entry) -> PathBuf {
        entry_dir(&self.paths, &entry.id).unwrap()
    }

    fn validate_trash(&self) -> Vec<(Option<String>, Option<Fix>, String)> {
        validate::check_all(&self.paths)
            .unwrap()
            .problems
            .into_iter()
            .filter(|problem| problem.code == Code::TrashInvalid)
            .map(|problem| (problem.environment, problem.fix, problem.message))
            .collect()
    }
}

#[test]
fn a_parseable_but_invalid_manifest_blocks_only_its_environment() {
    let f = Fixture::new();
    f.native("game");
    f.native("other");
    let entry = f.unsupported("game", false);
    f.native("game");

    let err = check_pending(&f.paths, "game", &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains(&f.dir(&entry).display().to_string()), "{err}");
    assert!(err.contains("unsupported manifest version 2"), "{err}");
    assert!(err.contains("blocks game"), "{err}");
    assert!(Environment::load(&f.paths, "game").is_err());
    check_pending(&f.paths, "other", &[]).unwrap();
    Environment::load(&f.paths, "other").unwrap();

    let scan = PendingScan::new(&f.paths);
    assert!(scan.check(&f.paths, "game", &[]).is_err());
    scan.check(&f.paths, "other", &[]).unwrap();
    Environment::load_with_pending(&f.paths, "other", &scan).unwrap();
    let found = scan.blocking_unreadable(&f.paths, "game").unwrap();
    assert_eq!(found.blocks.as_deref(), Some("game"));
    assert!(scan.blocking_unreadable(&f.paths, "other").is_none());
}

#[test]
fn a_parseable_but_invalid_manifest_blocks_the_paths_it_records() {
    let f = Fixture::new();
    f.wine("game");
    f.native("other");
    let entry = f.unsupported("game", true);
    let retained = f
        .paths
        .default_prefix_root()
        .join(".wineo-trash")
        .join(&entry.id)
        .join("prefix");
    assert!(retained.is_dir());

    let err = check_pending(&f.paths, "other", &[retained.as_path()]).unwrap_err();
    assert!(err.to_string().contains("blocks game"), "{err}");
    check_pending(
        &f.paths,
        "other",
        &[f.paths.default_prefix_root().join("other").as_path()],
    )
    .unwrap();
}

#[test]
fn every_check_failure_has_its_own_reason() {
    let f = Fixture::new();
    f.native("game");
    let entry = remove(&f.paths, "game", false).unwrap();
    let saved = read_manifest(&f.paths, &entry.id).unwrap();
    let reason = |change: &dyn Fn(&mut Manifest)| {
        let mut manifest = saved.clone();
        change(&mut manifest);
        let file = f.dir(&entry).join(MANIFEST);
        fs::write(file, toml::to_string(&manifest).unwrap()).unwrap();
        let err = read_manifest(&f.paths, &entry.id).unwrap_err();
        let found = err.downcast_ref::<Unverifiable>().unwrap();
        (found.blocks.clone(), found.reason.clone())
    };
    let game = Some("game".to_owned());
    assert_eq!(
        reason(&|m| m.id = "20200101T000000Z-00000000".into()),
        (
            game.clone(),
            "its manifest is for entry 20200101T000000Z-00000000".into()
        )
    );
    assert_eq!(
        reason(&|m| m.entry_identity.inode += 1),
        (game.clone(), "its folder's identity changed".into())
    );
    assert_eq!(
        reason(&|m| m.removed_at = "soon".into()),
        (game, "unreadable removal time `soon`".into())
    );
    assert_eq!(
        reason(&|m| m.name = "../bad".into()),
        (None, "invalid environment name `../bad`".into())
    );
}

#[test]
fn an_unparseable_manifest_blocks_every_environment() {
    let f = Fixture::new();
    f.native("game");
    f.native("other");
    let entry = f.garbled("game");

    let err = check_pending(&f.paths, "other", &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains(&f.dir(&entry).display().to_string()), "{err}");
    assert!(err.contains("manifest.toml can't be parsed"), "{err}");
    assert!(err.contains("blocks every environment"), "{err}");
    let scan = PendingScan::new(&f.paths);
    assert!(Environment::load_with_pending(&f.paths, "other", &scan).is_err());
    let found = scan.blocking_unreadable(&f.paths, "other").unwrap();
    assert_eq!(found.blocks, None);
}

#[test]
fn a_survey_lists_good_entries_next_to_an_unreadable_one() {
    let f = Fixture::new();
    f.native("good");
    f.native("bad");
    let good = remove(&f.paths, "good", false).unwrap();
    let bad = f.garbled("bad");

    assert!(list(&f.paths).is_err());
    let survey = survey(&f.paths).unwrap();
    assert_eq!(
        survey
            .entries
            .iter()
            .map(|entry| &entry.id)
            .collect::<Vec<_>>(),
        [&good.id]
    );
    assert_eq!(survey.unreadable.len(), 1);
    let unreadable = &survey.unreadable[0];
    assert_eq!(unreadable.id, bad.id);
    assert_eq!(unreadable.blocks, None);
    assert!(unreadable.reason.contains("can't be parsed"));
    assert!(unreadable.deletable.is_err());
}

#[test]
fn an_unreadable_entry_kept_inside_its_folder_can_be_pruned_by_id() {
    let f = Fixture::new();
    f.native("game");
    f.native("other");
    let entry = f.unsupported("game", false);
    let dir = f.dir(&entry);
    // A link inside the entry must go without its target going too.
    let outside = f.paths.trash_dir().parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    symlink(&outside, dir.join("link")).unwrap();

    assert!(survey(&f.paths).unwrap().unreadable[0].deletable.is_ok());
    let review = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    assert!(review.entries.is_empty());
    assert_eq!(review.unreadable.len(), 1);
    let outcomes = prune_reviewed(&f.paths, &review);
    assert_eq!(outcomes.len(), 1);
    outcomes[0].1.as_ref().unwrap();

    assert!(!dir.exists());
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
    assert_eq!(survey(&f.paths).unwrap(), Survey::default());
    Environment::load(&f.paths, "other").unwrap();
}

#[test]
fn a_reviewed_unreadable_entry_that_changes_is_not_deleted() {
    let f = Fixture::new();
    f.native("game");
    let entry = f.unsupported("game", false);
    let review = review_prune(&f.paths, Selector::Id(&entry.id)).unwrap();
    let mut manifest: Manifest =
        toml::from_str(&fs::read_to_string(f.dir(&entry).join(MANIFEST)).unwrap()).unwrap();
    manifest.version = VERSION + 2;
    write_manifest(&f.paths, &manifest).unwrap();

    let outcomes = prune_reviewed(&f.paths, &review);
    assert!(outcomes[0].1.is_err());
    assert!(f.dir(&entry).join(MANIFEST).exists());
}

#[test]
fn an_unreadable_entry_retained_outside_its_folder_is_not_deleted() {
    let f = Fixture::new();
    let prefix = f.wine("game");
    let entry = f.unsupported("game", true);

    let err = review_prune(&f.paths, Selector::Id(&entry.id))
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside its folder"), "{err}");
    assert!(
        err.contains(&format!(
            "inspect {} and remove it by hand",
            f.dir(&entry).display()
        )),
        "{err}"
    );
    assert!(delete_unreadable(&f.paths, &entry.id).is_err());
    assert!(f.dir(&entry).join(MANIFEST).exists());
    let retained = prefix
        .parent()
        .unwrap()
        .join(".wineo-trash")
        .join(&entry.id)
        .join("prefix");
    assert!(retained.join("system.reg").exists());
}

#[test]
fn an_unparseable_entry_is_not_deleted() {
    let f = Fixture::new();
    f.native("game");
    let entry = f.garbled("game");

    let err = review_prune(&f.paths, Selector::Id(&entry.id))
        .unwrap_err()
        .to_string();
    assert!(err.contains("its manifest can't be parsed"), "{err}");
    assert!(delete_unreadable(&f.paths, &entry.id).is_err());
    assert!(f.dir(&entry).join(MANIFEST).exists());
}

#[test]
fn an_unreadable_entry_is_left_out_of_batch_prunes() {
    let f = Fixture::new();
    f.native("game");
    let entry = f.unsupported("game", false);
    let review = review_prune(&f.paths, Selector::All).unwrap();
    assert!(review.entries.is_empty() && review.unreadable.is_empty());
    assert_eq!(review.errors.len(), 1);
    assert!(f.dir(&entry).exists());
}

#[test]
fn validate_reports_each_unreadable_entry_with_its_scope() {
    let f = Fixture::new();
    f.native("game");
    f.native("lost");
    f.wine("kept");
    f.native("other");
    let scoped = f.unsupported("game", false);
    let outside = f.unsupported("kept", true);
    let every = f.garbled("lost");

    let problems = f.validate_trash();
    assert_eq!(problems.len(), 3, "{problems:#?}");
    let find = |id: &str| {
        problems
            .iter()
            .find(|(_, _, message)| message.contains(id))
            .unwrap()
            .clone()
    };
    let (environment, fix, message) = find(&scoped.id);
    assert_eq!(environment.as_deref(), Some("game"));
    assert_eq!(
        fix,
        Some(Fix::DeleteTrashEntry {
            id: scoped.id.clone()
        })
    );
    assert!(
        message.contains("unsupported manifest version 2"),
        "{message}"
    );
    assert!(message.contains("blocks game"), "{message}");
    assert!(
        message.contains(&format!("`wineo prune {}`", scoped.id)),
        "{message}"
    );

    let (environment, fix, message) = find(&outside.id);
    assert_eq!(environment.as_deref(), Some("kept"));
    assert_eq!(fix, None);
    assert!(message.contains("remove it by hand"), "{message}");

    let (environment, fix, message) = find(&every.id);
    assert_eq!(environment, None);
    assert_eq!(fix, None);
    assert!(message.contains("blocks every environment"), "{message}");
    assert!(message.contains("can't be parsed"), "{message}");

    let selected = validate::check_environment(&f.paths, "other").unwrap();
    let scopes: Vec<_> = selected
        .problems
        .iter()
        .filter(|problem| problem.code == Code::TrashInvalid)
        .map(|problem| problem.environment.clone())
        .collect();
    assert_eq!(scopes, [None]);
}

#[test]
fn validate_fix_leaves_an_unreadable_entry_alone() {
    let f = Fixture::new();
    f.native("game");
    let entry = f.unsupported("game", false);
    let report = validate::check_all(&f.paths).unwrap();
    let fix = Fix::DeleteTrashEntry {
        id: entry.id.clone(),
    };
    assert!(report.problems.iter().any(|p| p.fix.as_ref() == Some(&fix)));
    assert!(!report.fixes().contains(&fix));
    let apply = |fix: &Fix| {
        validate::apply(
            &f.paths,
            fix,
            &crate::cancel::Token::new(),
            crate::winetricks::EnsureOutput::Stderr,
            |_| {},
            |_, _| unreachable!(),
        )
    };
    for fix in report.fixes() {
        let _ = apply(&fix);
    }
    assert!(apply(&fix).is_err());
    assert!(f.dir(&entry).join(MANIFEST).exists());

    validate::apply_single(
        &f.paths,
        &fix,
        &crate::cancel::Token::new(),
        crate::winetricks::EnsureOutput::Stderr,
        |_| {},
        |_, _| unreachable!(),
    )
    .unwrap();
    assert!(!f.dir(&entry).exists());
    assert!(f.validate_trash().is_empty());
}
