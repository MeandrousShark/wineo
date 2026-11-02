use std::ffi::OsString;
use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use super::super::{inspect, load};
use super::*;
use crate::library::FindingKind;

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        Self { _root: root, paths }
    }

    fn write(&self, text: &str) {
        fs::create_dir_all(self.paths.library_file().parent().unwrap()).unwrap();
        fs::write(self.paths.library_file(), text).unwrap();
    }

    fn read(&self) -> String {
        fs::read_to_string(self.paths.library_file()).unwrap()
    }

    fn reason(&self, id: &str) -> Option<Recorded> {
        load(&self.paths).steam.games.get(id).copied()
    }

    fn add(&self, id: &str, reason: Recorded) -> bool {
        record_games(&self.paths, &[id.to_owned()], reason).unwrap()
    }
}

#[test]
fn the_record_round_trips_and_keeps_comments_and_other_data() {
    let fixture = Fixture::new();
    let original =
        "# mine\nfuture_top = 1\n\n[view]\nsort = \"added\" # newest\n\n[future]\nx = 2\n";
    fixture.write(original);

    assert!(fixture.add("440", Recorded::Baseline));
    assert!(mark_baselined(&fixture.paths, "/games/one").unwrap());
    assert!(dismiss_game(&fixture.paths, "570").unwrap());

    let text = fixture.read();
    assert!(text.contains("# mine\nfuture_top = 1"), "{text}");
    assert!(text.contains("sort = \"added\" # newest"), "{text}");
    assert!(text.contains("[future]\nx = 2"), "{text}");
    assert!(text.contains("[steam]\n"), "{text}");
    assert!(text.contains("baselined = [\"/games/one\"]"), "{text}");
    assert!(text.contains("[steam.games]\n"), "{text}");
    assert!(text.contains("440 = \"baseline\"\n"), "{text}");
    assert!(text.contains("570 = \"dismissed\"\n"), "{text}");
    let steam = load(&fixture.paths).steam;
    assert!(steam.contains("440") && steam.contains("570") && !steam.contains("620"));
    assert!(steam.baselined.contains("/games/one"));
    assert_eq!(load(&fixture.paths).problem, None);
    let findings = inspect(&fixture.paths).findings;
    assert_eq!(findings.len(), 2, "{findings:?}"); // future_top and future
}

#[test]
fn a_no_op_leaves_the_file_byte_identical_and_a_legacy_file_unwritten() {
    let fixture = Fixture::new();
    // No schema_version: a write would add one.
    let original = "[steam]\nbaselined = [\"/a\"]\n[steam.games]\n1 = \"baseline\"\n2 = \"dismissed\"\n3 = \"imported\"\n# keep\n";
    fixture.write(original);

    assert!(!fixture.add("1", Recorded::Imported));
    assert!(!record_games(&fixture.paths, &[], Recorded::Baseline).unwrap());
    assert!(!dismiss_game(&fixture.paths, "1").unwrap());
    assert!(!dismiss_game(&fixture.paths, "2").unwrap());
    assert!(!finish_import(&fixture.paths, "2").unwrap());
    assert!(!finish_import(&fixture.paths, "3").unwrap());
    assert!(!release_claim(&fixture.paths, "1").unwrap());
    assert!(!release_claim(&fixture.paths, "99").unwrap());
    assert!(!mark_baselined(&fixture.paths, "/a").unwrap());
    assert!(!apply_changes(&fixture.paths, &RecordChanges::default()).unwrap());
    assert!(
        !apply_changes(
            &fixture.paths,
            &RecordChanges {
                baseline: vec!["1".into(), "2".into()],
                imported: vec!["3".into()],
                libraries: vec!["/a".into()],
            }
        )
        .unwrap()
    );
    assert_eq!(fixture.read(), original);
}

#[test]
fn a_write_with_no_change_creates_no_file() {
    let fixture = Fixture::new();
    assert!(!apply_changes(&fixture.paths, &RecordChanges::default()).unwrap());
    assert!(!release_claim(&fixture.paths, "1").unwrap());
    assert!(!fixture.paths.library_file().exists());
}

#[test]
fn a_baseline_never_replaces_a_decision() {
    let fixture = Fixture::new();
    assert!(dismiss_game(&fixture.paths, "1").unwrap());
    assert!(finish_import(&fixture.paths, "2").unwrap());
    fixture.add("3", Recorded::Claimed);
    for id in ["1", "2", "3"] {
        assert!(!fixture.add(id, Recorded::Baseline));
    }
    assert_eq!(fixture.reason("1"), Some(Recorded::Dismissed));
    assert_eq!(fixture.reason("2"), Some(Recorded::Imported));
    assert_eq!(fixture.reason("3"), Some(Recorded::Claimed));
}

#[test]
fn a_dismissal_and_a_finished_import_replace_a_claim() {
    let fixture = Fixture::new();
    for id in ["1", "2"] {
        assert_eq!(
            claim_game(&fixture.paths, id, |_| true).unwrap(),
            Claim::Claimed
        );
    }
    assert!(dismiss_game(&fixture.paths, "1").unwrap());
    assert!(finish_import(&fixture.paths, "2").unwrap());
    assert_eq!(fixture.reason("1"), Some(Recorded::Dismissed));
    assert_eq!(fixture.reason("2"), Some(Recorded::Imported));
}

#[test]
fn a_failed_imports_cleanup_removes_only_its_own_claim() {
    let fixture = Fixture::new();
    claim_game(&fixture.paths, "1", |_| true).unwrap();
    assert!(release_claim(&fixture.paths, "1").unwrap());
    assert_eq!(fixture.reason("1"), None);

    // The dismissal lands between the claim and the cleanup.
    claim_game(&fixture.paths, "2", |_| true).unwrap();
    dismiss_game(&fixture.paths, "2").unwrap();
    assert!(!release_claim(&fixture.paths, "2").unwrap());
    assert_eq!(fixture.reason("2"), Some(Recorded::Dismissed));
}

#[test]
fn a_claim_checks_the_file_under_its_lock() {
    let fixture = Fixture::new();
    fixture.write("[view]\nzoom = 120\n");
    assert_eq!(
        claim_game(&fixture.paths, "1", |saved| saved.view.zoom == 100).unwrap(),
        Claim::Refused
    );
    assert_eq!(fixture.reason("1"), None);
    assert_eq!(fixture.read(), "[view]\nzoom = 120\n");
    assert_eq!(
        claim_game(&fixture.paths, "1", |saved| saved.view.zoom == 120).unwrap(),
        Claim::Claimed
    );
    // A recorded game never reaches the check.
    assert_eq!(
        claim_game(&fixture.paths, "1", |_| panic!("not asked")).unwrap(),
        Claim::Recorded
    );
}

#[test]
fn only_one_of_many_claims_wins() {
    let fixture = Arc::new(Fixture::new());
    let barrier = Arc::new(Barrier::new(8));
    let claims: Vec<_> = (0..8)
        .map(|_| {
            let (fixture, barrier) = (Arc::clone(&fixture), Arc::clone(&barrier));
            thread::spawn(move || {
                barrier.wait();
                claim_game(&fixture.paths, "1", |_| true).unwrap()
            })
        })
        .collect();
    let won = claims
        .into_iter()
        .map(|claim| claim.join().unwrap())
        .filter(|claim| *claim == Claim::Claimed)
        .count();
    assert_eq!(won, 1);
}

#[test]
fn concurrent_additions_all_land() {
    let fixture = Arc::new(Fixture::new());
    let barrier = Arc::new(Barrier::new(8));
    let writers: Vec<_> = (0..8)
        .map(|writer| {
            let (fixture, barrier) = (Arc::clone(&fixture), Arc::clone(&barrier));
            thread::spawn(move || {
                barrier.wait();
                for game in 0..5 {
                    let id = (writer * 100 + game + 1).to_string();
                    record_games(&fixture.paths, &[id], Recorded::Baseline).unwrap();
                }
                mark_baselined(&fixture.paths, &format!("/games/{writer}")).unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let steam = load(&fixture.paths).steam;
    assert_eq!(steam.games.len(), 40);
    assert_eq!(steam.baselined.len(), 8);
}

#[test]
fn a_scans_changes_land_in_one_write() {
    let fixture = Fixture::new();
    fixture.add("3", Recorded::Dismissed);
    let changes = RecordChanges {
        baseline: vec!["1".into(), "3".into()],
        imported: vec!["2".into()],
        libraries: vec!["/a".into(), "/b".into()],
    };
    assert!(apply_changes(&fixture.paths, &changes).unwrap());
    assert_eq!(fixture.reason("1"), Some(Recorded::Baseline));
    assert_eq!(fixture.reason("2"), Some(Recorded::Imported));
    assert_eq!(fixture.reason("3"), Some(Recorded::Dismissed));
    assert_eq!(load(&fixture.paths).steam.baselined.len(), 2);
}

#[test]
fn a_write_to_an_unreadable_file_fails_and_keeps_it() {
    let fixture = Fixture::new();
    fixture.write("not toml [");
    assert!(dismiss_game(&fixture.paths, "1").is_err());
    assert!(record_games(&fixture.paths, &["1".to_owned()], Recorded::Baseline).is_err());
    assert_eq!(fixture.read(), "not toml [");
}

#[test]
fn invalid_entries_read_as_absent_warn_and_get_replaced() {
    let fixture = Fixture::new();
    fixture.write(
        "[steam]\nbaselined = [\"/a\", 5]\nfuture = 1\n[steam.games]\n1 = \"nonsense\"\nabc = \"baseline\"\n2 = \"imported\"\n",
    );

    let steam = load(&fixture.paths).steam;
    assert_eq!(steam.games.len(), 1);
    assert_eq!(steam.baselined.len(), 1);
    let findings = inspect(&fixture.paths).findings;
    let kinds: Vec<_> = findings.iter().map(|finding| finding.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == FindingKind::FieldInvalid)
            .count(),
        3,
        "{findings:?}"
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == FindingKind::KeyUnknown)
            .count(),
        1,
        "{findings:?}"
    );

    assert!(fixture.add("1", Recorded::Baseline));
    assert_eq!(fixture.reason("1"), Some(Recorded::Baseline));
    assert!(fixture.read().contains("future = 1"));
}

#[test]
fn a_wrong_shape_warns_and_a_writer_makes_it_right() {
    let fixture = Fixture::new();
    fixture.write("steam = 5\n");
    assert_eq!(load(&fixture.paths).steam, SteamRecord::default());
    assert_eq!(inspect(&fixture.paths).findings.len(), 1);
    assert!(fixture.add("1", Recorded::Baseline));
    assert_eq!(fixture.reason("1"), Some(Recorded::Baseline));

    fixture.write("[steam]\ngames = 5\nbaselined = \"/a\"\n");
    assert_eq!(inspect(&fixture.paths).findings.len(), 2);
    assert!(mark_baselined(&fixture.paths, "/b").unwrap());
    assert!(fixture.add("1", Recorded::Baseline));
    let steam = load(&fixture.paths).steam;
    assert!(steam.contains("1") && steam.baselined.contains("/b"));
}

#[test]
fn a_hand_written_inline_table_still_takes_entries() {
    let fixture = Fixture::new();
    fixture.write("steam = { games = { 1 = \"baseline\" } }\n");
    assert_eq!(fixture.reason("1"), Some(Recorded::Baseline));
    assert!(fixture.add("2", Recorded::Dismissed));
    assert!(mark_baselined(&fixture.paths, "/a").unwrap());
    let saved = load(&fixture.paths);
    assert_eq!(saved.steam.games.len(), 2);
    assert_eq!(saved.problem, None);
}

#[test]
fn an_id_that_is_not_a_number_is_never_written() {
    let fixture = Fixture::new();
    assert!(!fixture.add("../x", Recorded::Baseline));
    assert!(!dismiss_game(&fixture.paths, "abc").unwrap());
    assert!(!fixture.paths.library_file().exists());
}
