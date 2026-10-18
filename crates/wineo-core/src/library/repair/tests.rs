use std::ffi::OsString;

use super::*;
use crate::library::{SortKey, load};

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
        fs::create_dir_all(self.dir()).unwrap();
        fs::write(self.paths.library_file(), text).unwrap();
    }

    fn read(&self) -> String {
        fs::read_to_string(self.paths.library_file()).unwrap()
    }

    fn dir(&self) -> PathBuf {
        self.paths.library_file().parent().unwrap().to_owned()
    }

    fn backup(&self, suffix: &str) -> PathBuf {
        self.dir().join(format!("library.toml.broken-{suffix}"))
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

const STAMP: &str = "2026-10-17T19-20-05";

// The second `[environments.hades]` makes the file unparseable, but every
// line still reads on its own.
const RECOVERABLE: &str = r#"schema_version = 1

[view]
sort = "playtime"
descending = true

[environments.hades]
favorite = true # starred by hand
added = 2026-10-17T18:00:00Z

[environments."old demo"]
hidden = true

[environments.hades]
future = "kept"
"#;

const DAMAGED: &str = r#"[view]
sort = "bogus"
descending = true

[environments.hades]
favorite = "yes"
hidden = true

[environments.celeste]
favorite = true

[environments.gone]
favorite =
added = 2026-10-17

[broken header
favorite = true
"#;

#[test]
fn backs_up_the_original_and_recovers_every_recognized_entry() {
    let fixture = Fixture::new();
    fixture.write(RECOVERABLE);

    let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    assert_eq!(
        report,
        RepairReport {
            entries: 2,
            full: 2,
            partly: 0,
            view: ViewRecovery::Recovered,
            dropped_text: false,
            backup: fixture.backup(STAMP),
        }
    );
    assert_eq!(
        fs::read_to_string(fixture.backup(STAMP)).unwrap(),
        RECOVERABLE
    );
    assert_eq!(
        report.to_string(),
        format!(
            "Recovered all 2 recognized entries. Sort settings recovered.\nOriginal saved as library.toml.broken-{STAMP}."
        )
    );
    assert_eq!(
        fixture.read(),
        r#"schema_version = 1

[view]
sort = "playtime"
descending = true

[environments.hades]
favorite = true # starred by hand
added = 2026-10-17T18:00:00Z
future = "kept"

[environments."old demo"]
hidden = true
"#
    );
    let library = load(&fixture.paths);
    assert_eq!(library.problem, None);
    assert_eq!(library.view.sort, SortKey::Playtime);
    assert!(library.view.descending);
    assert!(library.environments["hades"].favorite);
    assert!(library.environments["hades"].added.is_some());
    assert!(library.environments["old demo"].hidden);
}

#[test]
fn counts_partly_and_unrecovered_entries_and_skips_invalid_values() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    // hades keeps hidden but not `favorite = "yes"`; gone's `favorite =`
    // and date-only `added` are recognized but neither is kept; the text
    // under the broken header can't be attributed.
    assert_eq!(
        report,
        RepairReport {
            entries: 3,
            full: 1,
            partly: 1,
            view: ViewRecovery::Partly,
            dropped_text: true,
            backup: fixture.backup(STAMP),
        }
    );
    assert_eq!(report.lost(), 1);
    assert_eq!(
        report.to_string(),
        format!(
            "Recovered 1 of 3 recognized entries fully and 1 partly. Some data could not be recovered. Sort settings partly recovered.\nOriginal saved as library.toml.broken-{STAMP}."
        )
    );
    let text = fixture.read();
    assert!(!text.contains("yes"), "{text}");
    assert!(!text.contains("bogus"), "{text}");
    let library = load(&fixture.paths);
    assert_eq!(library.problem, None);
    assert!(!library.environments["hades"].favorite);
    assert!(library.environments["hades"].hidden);
    assert!(library.environments["celeste"].favorite);
    assert_eq!(
        library.environments["gone"],
        crate::library::Entry::default()
    );
    assert_eq!(library.view.sort, SortKey::Name);
    assert!(library.view.descending);
    assert_eq!(fs::read_to_string(fixture.backup(STAMP)).unwrap(), DAMAGED);
}

#[test]
fn reports_dropped_text_even_when_every_entry_came_back() {
    let fixture = Fixture::new();
    fixture.write("this is not toml\n\n[environments.hades]\nfavorite = true\n");

    let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    assert_eq!((report.entries, report.full), (1, 1));
    assert!(report.dropped_text);
    assert_eq!(report.view, ViewRecovery::Absent);
    assert_eq!(
        report.to_string(),
        format!(
            "Recovered the one recognized entry. Some data could not be recovered.\nOriginal saved as library.toml.broken-{STAMP}."
        )
    );
    assert!(load(&fixture.paths).environments["hades"].favorite);
}

#[test]
fn a_line_too_damaged_to_be_a_field_is_dropped_text_not_a_lost_field() {
    let fixture = Fixture::new();
    fixture.write("[environments.hades]\nfavorite = true\n\"unterminated\n");

    let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (1, 1, 0));
    assert!(report.dropped_text);
}

#[test]
fn a_failed_backup_aborts_and_leaves_the_original_untouched() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    // A stamp naming a missing directory makes only the backup fail, so the
    // file would otherwise be writable.
    let error = repair_at(&fixture.paths, "missing/dir").unwrap_err();

    assert!(error.to_string().contains("failed to back up"), "{error}");
    assert_eq!(fixture.read(), DAMAGED);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn an_unwritable_directory_aborts_before_anything_changes() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.write(DAMAGED);
    fs::set_permissions(fixture.dir(), fs::Permissions::from_mode(0o500)).unwrap();

    let result = repair_at(&fixture.paths, STAMP);

    fs::set_permissions(fixture.dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fixture.read(), DAMAGED);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn two_repairs_in_the_same_second_keep_both_backups() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);
    let first = repair_at(&fixture.paths, STAMP).unwrap().unwrap();
    let second_original = "[environments.celeste]\nhidden = true\n[environments.celeste]\n";
    fixture.write(second_original);

    let second = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    assert_eq!(first.backup, fixture.backup(STAMP));
    assert_eq!(second.backup, fixture.backup(&format!("{STAMP}-2")));
    assert_eq!(fs::read_to_string(&first.backup).unwrap(), DAMAGED);
    assert_eq!(fs::read_to_string(&second.backup).unwrap(), second_original);
    assert!(
        second
            .to_string()
            .ends_with(&format!("library.toml.broken-{STAMP}-2."))
    );
}

#[test]
fn a_readable_or_missing_file_is_left_alone() {
    let fixture = Fixture::new();
    assert_eq!(repair_at(&fixture.paths, STAMP).unwrap(), None);
    assert!(!fixture.paths.library_file().exists());

    let readable = "schema_version = 1\n\n[environments.hades]\nfavorite = \"yes\"\n";
    fixture.write(readable);
    assert_eq!(repair_at(&fixture.paths, STAMP).unwrap(), None);
    assert_eq!(fixture.read(), readable);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn a_newer_schema_version_is_refused() {
    let fixture = Fixture::new();
    let newer = "schema_version = 2\n\n[environments.hades]\nfavorite = true\n";
    fixture.write(newer);

    let error = repair_at(&fixture.paths, STAMP).unwrap_err();

    assert!(matches!(error, Error::SchemaVersion { .. }), "{error}");
    assert_eq!(fixture.read(), newer);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn an_unsupported_older_or_mistyped_schema_version_is_repaired() {
    for version in ["0", "\"1\""] {
        let fixture = Fixture::new();
        fixture.write(&format!(
            "schema_version = {version}\n\n[environments.hades]\nfavorite = true\n"
        ));

        let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

        assert!(report.complete(), "{report:?}");
        assert!(fixture.read().starts_with("schema_version = 1\n"));
        assert!(load(&fixture.paths).environments["hades"].favorite);
    }
}

#[test]
fn invalid_utf8_is_salvaged_around() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.dir()).unwrap();
    let mut bytes = b"[environments.hades]\nfavorite = true\nnote = \"".to_vec();
    bytes.extend([0xff, 0xfe]);
    bytes.extend(b"\"\n");
    fs::write(fixture.paths.library_file(), &bytes).unwrap();

    let report = repair_at(&fixture.paths, STAMP).unwrap().unwrap();

    assert_eq!(fs::read(&report.backup).unwrap(), bytes);
    assert!(load(&fixture.paths).environments["hades"].favorite);
}

#[test]
fn repair_names_the_backup_with_the_local_time() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    let report = repair(&fixture.paths).unwrap().unwrap();

    let name = report.backup.file_name().unwrap().to_str().unwrap();
    let stamp = name.strip_prefix("library.toml.broken-").unwrap();
    assert!(
        jiff::civil::DateTime::strptime("%Y-%m-%dT%H-%M-%S", stamp).is_ok(),
        "{name}"
    );
}

#[test]
fn wording_for_no_entries_and_lost_view() {
    let report = RepairReport {
        entries: 0,
        full: 0,
        partly: 0,
        view: ViewRecovery::Lost,
        dropped_text: false,
        backup: PathBuf::from("/x/library.toml.broken-1"),
    };
    assert_eq!(
        report.to_string(),
        "No library entries were recognized. Some data could not be recovered. Sort settings could not be recovered.\nOriginal saved as library.toml.broken-1."
    );
    let report = RepairReport {
        entries: 2,
        full: 0,
        partly: 0,
        view: ViewRecovery::Absent,
        dropped_text: false,
        backup: PathBuf::from("/x/library.toml.broken-1"),
    };
    assert_eq!(
        report.to_string(),
        "Recovered 0 of 2 recognized entries fully. Some data could not be recovered.\nOriginal saved as library.toml.broken-1."
    );
}
