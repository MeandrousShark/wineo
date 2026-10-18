use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;

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
}

#[test]
fn loads_missing_library_as_empty_without_creating_it() {
    let fixture = Fixture::new();
    assert_eq!(load(&fixture.paths), Library::default());
    assert!(!fixture.paths.library_file().exists());
}

#[test]
fn round_trips_known_fields_and_preserves_future_keys() {
    let fixture = Fixture::new();
    fixture.write(
        r#"schema_version = 1
future_top = "kept"

[view]
sort = "last-played"
descending = true
favorites_first = true
future_view = 7

[environments.hades]
favorite = true
hidden = true
added = 2026-10-17T19:20:05Z
future_entry = "kept"
"#,
    );

    let library = load(&fixture.paths);
    assert_eq!(library.view.sort, SortKey::LastPlayed);
    assert!(library.view.descending);
    assert!(library.view.favorites_first);
    assert_eq!(
        library.environments["hades"],
        Entry {
            favorite: true,
            hidden: true,
            added: Some("2026-10-17T19:20:05Z".parse().unwrap()),
        }
    );
    assert_eq!(library.problem, None);

    assert!(set_favorite(&fixture.paths, "hades", false).unwrap());
    let written = fixture.read();
    assert!(written.contains("future_top = \"kept\""));
    assert!(written.contains("future_view = 7"));
    assert!(written.contains("future_entry = \"kept\""));
    assert!(written.contains("sort = \"last-played\""));
    assert!(written.contains("favorite = false"));
    assert!(load(&fixture.paths).environments["hades"].hidden);
}

#[test]
fn bad_fields_fall_back_independently_and_survive_unrelated_edits() {
    let fixture = Fixture::new();
    fixture.write(
        r#"schema_version = 1

[view]
sort = "future-sort" # keep sort comment
descending = true
favorites_first = "yes"
future_view = "kept"

[environments.hades]
favorite = "yes" # keep favorite comment
hidden = true
added = 2026-10-17T19:20:05Z
future_entry = "kept"
"#,
    );

    let library = load(&fixture.paths);
    assert_eq!(library.view.sort, SortKey::Name);
    assert!(library.view.descending);
    assert!(!library.view.favorites_first);
    assert!(!library.environments["hades"].favorite);
    assert!(library.environments["hades"].hidden);
    assert_eq!(
        library.environments["hades"].added,
        Some("2026-10-17T19:20:05Z".parse().unwrap())
    );

    assert!(set_favorites_first(&fixture.paths, true).unwrap());
    let after_unrelated_edit = fixture.read();
    assert!(after_unrelated_edit.contains("sort = \"future-sort\""));
    assert!(after_unrelated_edit.contains("favorites_first = true"));
    assert!(after_unrelated_edit.contains("favorite = \"yes\""));
    assert!(after_unrelated_edit.contains("added = 2026-10-17T19:20:05Z"));
    assert!(after_unrelated_edit.contains("future_entry = \"kept\""));

    assert!(set_favorite(&fixture.paths, "hades", false).unwrap());
    let after_bad_field_edit = fixture.read();
    assert!(after_bad_field_edit.contains("favorite = false"));
    assert!(after_bad_field_edit.contains("favorite = false # keep favorite comment"));
    assert!(after_bad_field_edit.contains("sort = \"future-sort\""));
    assert!(after_bad_field_edit.contains("hidden = true"));
    assert!(after_bad_field_edit.contains("future_entry = \"kept\""));

    assert!(set_sort(&fixture.paths, SortKey::Name).unwrap());
    assert!(
        fixture
            .read()
            .contains("sort = \"name\" # keep sort comment")
    );
}

#[test]
fn no_op_edits_and_reads_do_not_replace_the_file() {
    let fixture = Fixture::new();
    fixture
        .write("# keep formatting\nschema_version = 1\n\n[environments.hades]\nhidden = false\n");
    let path = fixture.paths.library_file();
    let before = fs::metadata(&path).unwrap().ino();
    let bytes = fs::read(&path).unwrap();

    let _ = load(&fixture.paths);
    assert!(!set_favorite(&fixture.paths, "hades", false).unwrap());
    assert!(!set_hidden(&fixture.paths, "hades", false).unwrap());
    assert!(!set_sort(&fixture.paths, SortKey::Name).unwrap());
    assert!(!set_descending(&fixture.paths, false).unwrap());
    assert!(!set_favorites_first(&fixture.paths, false).unwrap());
    assert!(!remove(&fixture.paths, "missing").unwrap());

    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(path).unwrap().ino(), before);

    let missing = Fixture::new();
    assert!(!set_favorite(&missing.paths, "hades", false).unwrap());
    assert!(!missing.paths.library_file().exists());
}

#[test]
fn a_real_edit_creates_library_with_schema_version_one() {
    let fixture = Fixture::new();
    assert!(set_favorite(&fixture.paths, "hades", true).unwrap());
    let text = fixture.read();
    assert!(text.starts_with("schema_version = 1"));
    assert!(text.contains("favorite = true"));
}

#[test]
fn concurrent_writers_both_land() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(3));
    let writers = ["hades", "celeste"].map(|name| {
        let paths = fixture.paths.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            set_favorite(&paths, name, true).unwrap()
        })
    });
    barrier.wait();
    assert!(writers.into_iter().all(|writer| writer.join().unwrap()));

    let library = load(&fixture.paths);
    assert!(library.environments["hades"].favorite);
    assert!(library.environments["celeste"].favorite);
}

#[test]
fn favorite_edit_uses_the_fresh_view_after_an_external_change() {
    let fixture = Fixture::new();
    fixture.write("schema_version = 1\n[view]\nsort = \"added\"\ndescending = true\n");

    // Simulate another process saving its own view before the favorite edit.
    fs::write(
        fixture.paths.library_file(),
        "schema_version = 1\n[view]\nsort = \"playtime\"\ndescending = false\nfavorites_first = true\n",
    )
    .unwrap();
    assert!(set_favorite(&fixture.paths, "hades", true).unwrap());

    let library = load(&fixture.paths);
    assert_eq!(library.view.sort, SortKey::Playtime);
    assert!(!library.view.descending);
    assert!(library.view.favorites_first);
    assert!(library.environments["hades"].favorite);
}

#[test]
fn syntax_errors_degrade_reads_and_refuse_writes_without_changing_bytes() {
    let fixture = Fixture::new();
    let original = "schema_version = 1\n[view]\nsort = \"name\"\nthis is broken\n";
    fixture.write(original);

    let library = load(&fixture.paths);
    assert_eq!(library.view, View::default());
    assert!(library.environments.is_empty());
    let problem = library.problem.unwrap();
    assert_eq!(problem.line, Some(4));
    assert!(!problem.newer_version);

    let error = set_favorite(&fixture.paths, "hades", true).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup."
    );
    assert!(crate::error::chain(&error).contains("TOML parse error at line 4"));
    assert_eq!(fixture.read(), original);
}

#[test]
fn unsupported_schema_versions_degrade_reads_and_refuse_writes() {
    for original in [
        "schema_version = 0\n[environments.hades]\nfavorite = true\n",
        "schema_version = 2\n[environments.hades]\nfavorite = true\n",
        "schema_version = \"one\"\n[environments.hades]\nfavorite = true\n",
    ] {
        let fixture = Fixture::new();
        fixture.write(original);

        let library = load(&fixture.paths);
        assert_eq!(
            library,
            Library {
                problem: Some(Problem {
                    reason: "library.toml was written by a newer wineo; update wineo to use it."
                        .to_owned(),
                    line: None,
                    newer_version: true,
                }),
                ..Library::default()
            }
        );
        let error = set_hidden(&fixture.paths, "hades", true).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup."
        );
        assert!(crate::error::chain(&error).contains("unsupported schema_version"));
        assert_eq!(fixture.read(), original);
    }
}

#[test]
fn missing_schema_version_reads_as_one_and_is_added_on_a_real_edit() {
    let fixture = Fixture::new();
    fixture.write("[environments.hades]\nhidden = true\n");

    assert!(load(&fixture.paths).environments["hades"].hidden);
    assert!(set_favorite(&fixture.paths, "hades", true).unwrap());
    let text = fixture.read();
    assert!(text.starts_with("schema_version = 1"));
    assert!(text.contains("hidden = true"));
    assert!(text.contains("favorite = true"));
}

#[test]
fn lifecycle_helpers_replace_stamp_and_remove_entries() {
    let fixture = Fixture::new();
    fixture.write("schema_version = 1\n[environments.hades]\nfavorite = true\nhidden = true\n");
    assert!(fresh(&fixture.paths, "hades").unwrap());
    let fresh_entry = &load(&fixture.paths).environments["hades"];
    assert!(!fresh_entry.favorite);
    assert!(!fresh_entry.hidden);
    assert!(fresh_entry.added.is_some());

    let replacement = Entry {
        favorite: true,
        hidden: false,
        added: Some("2026-10-17T19:20:05Z".parse().unwrap()),
    };
    assert!(put(&fixture.paths, "hades", Some(&replacement)).unwrap());
    assert_eq!(load(&fixture.paths).environments["hades"], replacement);
    assert!(remove(&fixture.paths, "hades").unwrap());
    assert!(!load(&fixture.paths).environments.contains_key("hades"));
}

#[test]
fn added_stamp_changes_only_the_added_field() {
    let fixture = Fixture::new();
    fixture.write(
        "schema_version = 1\n[environments.hades]\nfavorite = true\nhidden = true\nfuture = \"kept\"\n",
    );
    assert!(stamp_added(&fixture.paths, "hades").unwrap());
    let entry = &load(&fixture.paths).environments["hades"];
    assert!(entry.favorite);
    assert!(entry.hidden);
    assert!(entry.added.is_some());
    assert!(fixture.read().contains("future = \"kept\""));
}

#[test]
fn library_file_is_loaded_from_the_selected_config_root() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .paths
            .library_file()
            .file_name()
            .and_then(|name| name.to_str()),
        Some("library.toml")
    );
    assert!(
        fixture
            .paths
            .library_file()
            .starts_with(fixture.paths.global_config().parent().unwrap())
    );
    assert!(Path::new(&fixture.paths.library_file()).is_absolute());
}
