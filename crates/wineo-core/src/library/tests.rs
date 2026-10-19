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
fn a_first_write_has_no_empty_parent_header() {
    let fixture = Fixture::new();
    assert!(set_favorite(&fixture.paths, "hades", true).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n\n[environments.hades]\nfavorite = true\n"
    );
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
fn set_order_writes_the_key_and_direction_in_one_edit() {
    let fixture = Fixture::new();
    fixture.write("schema_version = 1\n\n[view]\nsort = \"name\" # keep\n");
    assert!(set_order(&fixture.paths, SortKey::Playtime, true).unwrap());
    let view = load(&fixture.paths).view;
    assert_eq!((view.sort, view.descending), (SortKey::Playtime, true));
    assert!(fixture.read().contains("sort = \"playtime\" # keep"));

    assert!(!set_order(&fixture.paths, SortKey::Playtime, true).unwrap());
    assert!(set_order(&fixture.paths, SortKey::Playtime, false).unwrap());
    assert!(set_order(&fixture.paths, SortKey::Name, false).unwrap());
    let view = load(&fixture.paths).view;
    assert_eq!((view.sort, view.descending), (SortKey::Name, false));
}

#[test]
fn a_saved_source_sort_reads_as_name_and_keeps_the_direction() {
    let fixture = Fixture::new();
    fixture.write("schema_version = 1\n\n[view]\nsort = \"source\"\ndescending = true\n");
    let view = load(&fixture.paths).view;
    assert_eq!((view.sort, view.descending), (SortKey::Name, true));
    assert_eq!(
        fixture.read().matches("source").count(),
        1,
        "reads don't rewrite"
    );
}

#[test]
fn only_name_starts_ascending() {
    for sort in SortKey::ALL {
        assert_eq!(sort.starts_descending(), sort != SortKey::Name, "{sort:?}");
    }
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
    // Only a higher version is a newer wineo's file; 0 or a value of the
    // wrong type is broken, and Checkup can repair it.
    for (original, newer, reason) in [
        (
            "schema_version = 0\n[environments.hades]\nfavorite = true\n",
            false,
            "unsupported schema_version 0",
        ),
        (
            "schema_version = 2\n[environments.hades]\nfavorite = true\n",
            true,
            "library.toml was written by a newer wineo; update wineo to use it.",
        ),
        (
            "schema_version = \"one\"\n[environments.hades]\nfavorite = true\n",
            false,
            "unsupported schema_version \"one\"",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.write(original);

        let library = load(&fixture.paths);
        assert_eq!(
            library,
            Library {
                problem: Some(Problem {
                    reason: reason.to_owned(),
                    line: None,
                    newer_version: newer,
                }),
                ..Library::default()
            }
        );
        let error = set_hidden(&fixture.paths, "hades", true).unwrap_err();
        assert!(matches!(
            error,
            crate::error::Error::LibraryUnreadable { newer_version, .. } if newer_version == newer
        ));
        assert_eq!(
            error.to_string(),
            if newer {
                "library.toml was written by a newer wineo; update wineo to use it."
            } else {
                "Can't save to library.toml: it can't be read. Fix it, or repair it in Checkup."
            }
        );
        assert!(crate::error::chain(&error).contains("unsupported schema_version"));
        assert_eq!(fixture.read(), original);
    }
}

/// What `load` and `validate` each say about a file whose version is off.
fn load_and_validate(fixture: &Fixture, text: &str) -> (Problem, check::Unreadable) {
    fixture.write(text);
    let problem = load(&fixture.paths).problem.expect("load finds a problem");
    let unreadable = inspect(&fixture.paths)
        .unreadable
        .expect("validate finds a problem");
    (problem, unreadable)
}

#[test]
fn load_and_validate_classify_schema_versions_alike() {
    let fixture = Fixture::new();
    for (text, newer) in [
        ("schema_version = 0\n", false),
        ("schema_version = \"one\"\n", false),
        ("schema_version = 1.5\n", false),
        ("schema_version = 2\n", true),
        // Doesn't parse, but says a newer wineo wrote it.
        ("schema_version = 2\nthis is broken\n", true),
        // Doesn't parse and says nothing newer.
        ("schema_version = 1\nthis is broken\n", false),
        ("schema_version = 0\nthis is broken\n", false),
    ] {
        let (problem, unreadable) = load_and_validate(&fixture, text);
        assert_eq!(problem.newer_version, newer, "{text:?}");
        assert_eq!(unreadable.newer, newer, "{text:?}");
        assert_eq!(problem.message(), unreadable.message, "{text:?}");
        // A refused write agrees too.
        let error = set_favorite(&fixture.paths, "hades", true).unwrap_err();
        assert!(
            matches!(
                error,
                crate::error::Error::LibraryUnreadable { newer_version, .. } if newer_version == newer
            ),
            "{text:?}: {error}"
        );
    }
    let (problem, _) = load_and_validate(&fixture, "schema_version = 0\n");
    assert_eq!(
        problem.message(),
        "library.toml can't be read (unsupported schema_version 0)"
    );
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

#[test]
fn a_problem_reads_like_its_validate_message() {
    let fixture = Fixture::new();
    fixture.write("schema_version = 1\n[view]\nsort = \"name\"\nthis is broken\n");
    let message = load(&fixture.paths).problem.unwrap().message();
    assert!(
        message.starts_with("library.toml can't be read (line 4: "),
        "{message}"
    );
    assert_eq!(
        Some(message),
        inspect(&fixture.paths)
            .unreadable
            .map(|unreadable| unreadable.message)
    );

    fixture.write("schema_version = 2\n");
    assert_eq!(
        load(&fixture.paths).problem.unwrap().message(),
        "library.toml was written by a newer wineo; update wineo to use it."
    );
}

fn make_config(paths: &Paths, name: &str) {
    let path = paths.environment_config(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "").unwrap();
}

#[test]
fn a_guarded_write_sets_the_field_for_the_same_environment() {
    let fixture = Fixture::new();
    let added: Timestamp = "2026-10-17T19:20:05Z".parse().unwrap();
    fixture.write("[environments.hades]\nadded = 2026-10-17T19:20:05Z\n");
    make_config(&fixture.paths, "hades");
    assert_eq!(
        set_favorite_for(&fixture.paths, "hades", Some(added), true).unwrap(),
        Guarded::Set(true)
    );
    assert_eq!(
        set_favorite_for(&fixture.paths, "hades", Some(added), true).unwrap(),
        Guarded::Set(false)
    );
    assert_eq!(
        set_hidden_for(&fixture.paths, "hades", Some(added), true).unwrap(),
        Guarded::Set(true)
    );
    let entry = &load(&fixture.paths).environments["hades"];
    assert!(entry.favorite && entry.hidden);

    // An environment from before `added` was recorded has none.
    make_config(&fixture.paths, "old");
    assert_eq!(
        set_hidden_for(&fixture.paths, "old", None, true).unwrap(),
        Guarded::Set(true)
    );
    assert!(load(&fixture.paths).environments["old"].hidden);
}

#[test]
fn a_guarded_write_for_a_removed_environment_writes_nothing() {
    let fixture = Fixture::new();
    // `rm` already cleaned up its entry, and its config is gone.
    fixture.write("schema_version = 1\n");
    let before = fixture.read();
    assert_eq!(
        set_favorite_for(&fixture.paths, "hades", None, true).unwrap(),
        Guarded::Gone
    );
    // Or the entry is still there, but the config isn't.
    fixture.write("[environments.hades]\nfavorite = true\n");
    let lingering = fixture.read();
    assert_eq!(
        set_hidden_for(&fixture.paths, "hades", None, true).unwrap(),
        Guarded::Gone
    );
    assert_eq!(fixture.read(), lingering);
    assert_ne!(before, lingering);
}

#[test]
fn a_guarded_write_for_a_recreated_environment_writes_nothing() {
    let fixture = Fixture::new();
    make_config(&fixture.paths, "hades");
    // A new hades was made after the old one's entry was read.
    let text = "[environments.hades]\nadded = 2026-10-18T09:00:00Z\n";
    fixture.write(text);
    let old: Timestamp = "2026-10-17T19:20:05Z".parse().unwrap();
    for added in [Some(old), None] {
        assert_eq!(
            set_favorite_for(&fixture.paths, "hades", added, true).unwrap(),
            Guarded::Gone
        );
    }
    assert_eq!(fixture.read(), text);
}

#[test]
fn a_guarded_write_to_a_broken_file_is_refused() {
    let fixture = Fixture::new();
    make_config(&fixture.paths, "hades");
    fixture.write("[environments.hades\n");
    let error = set_favorite_for(&fixture.paths, "hades", None, true).unwrap_err();
    assert!(matches!(error, Error::LibraryUnreadable { .. }), "{error}");
    assert_eq!(fixture.read(), "[environments.hades\n");
}

#[test]
fn a_comment_line_above_a_written_key_stays() {
    let fixture = Fixture::new();
    fixture.write(
        "schema_version = 1\n\n[view]\n# newest first\ndescending = true\n\n[environments.hades]\n# my favorite\nfavorite = false\n",
    );

    assert!(set_descending(&fixture.paths, false).unwrap());
    assert!(set_favorite(&fixture.paths, "hades", true).unwrap());
    let text = fixture.read();
    assert!(
        text.contains("# newest first\ndescending = false"),
        "{text}"
    );
    assert!(text.contains("# my favorite\nfavorite = true"), "{text}");
}

fn zoom_of(fixture: &Fixture, text: &str) -> u16 {
    fixture.write(&format!("schema_version = 1\n\n[view]\n{text}\n"));
    load(&fixture.paths).view.zoom
}

#[test]
fn zoom_and_poster_only_default_when_absent() {
    let fixture = Fixture::new();
    let view = load(&fixture.paths).view;
    assert_eq!((view.zoom, view.poster_only), (100, false));
    assert_eq!(view, View::default());
    assert_eq!(ZOOM_DEFAULT, 100);

    fixture.write("schema_version = 1\n\n[view]\nsort = \"name\"\n");
    let view = load(&fixture.paths).view;
    assert_eq!((view.zoom, view.poster_only), (100, false));
}

#[test]
fn any_whole_zoom_in_range_reads_back() {
    let fixture = Fixture::new();
    for zoom in [60, 61, 99, 100, 125, 133, 199, 200] {
        assert_eq!(zoom_of(&fixture, &format!("zoom = {zoom}")), zoom);
    }
    fixture.write("schema_version = 1\n\n[view]\nzoom = 75\nposter_only = true\n");
    let view = load(&fixture.paths).view;
    assert_eq!((view.zoom, view.poster_only), (75, true));
}

#[test]
fn a_bad_zoom_reads_as_the_default_and_touches_no_other_field() {
    let fixture = Fixture::new();
    for text in [
        "zoom = 59",
        "zoom = 201",
        "zoom = 0",
        "zoom = -100",
        "zoom = 70000",
        "zoom = 1.25",
        "zoom = 125.0",
        "zoom = \"125\"",
        "zoom = true",
        "zoom = [125]",
    ] {
        assert_eq!(zoom_of(&fixture, text), 100, "{text}");
    }

    fixture.write(
        "schema_version = 1\n\n[view]\nsort = \"playtime\"\nzoom = 500\ndescending = true\nposter_only = \"yes\"\nfavorites_first = true\n",
    );
    let library = load(&fixture.paths);
    assert_eq!(library.problem, None);
    assert_eq!(
        library.view,
        View {
            sort: SortKey::Playtime,
            descending: true,
            favorites_first: true,
            zoom: 100,
            poster_only: false,
        }
    );

    fixture.write("schema_version = 1\n\n[view]\nzoom = 150\nposter_only = 1\n");
    let view = load(&fixture.paths).view;
    assert_eq!((view.zoom, view.poster_only), (150, false));
}

#[test]
fn set_zoom_writes_replaces_and_clamps() {
    let fixture = Fixture::new();
    assert!(set_zoom(&fixture.paths, 125).unwrap());
    assert!(fixture.read().contains("zoom = 125"), "{}", fixture.read());
    assert_eq!(load(&fixture.paths).view.zoom, 125);

    assert!(set_zoom(&fixture.paths, 80).unwrap());
    assert_eq!(load(&fixture.paths).view.zoom, 80);

    assert!(set_zoom(&fixture.paths, 5000).unwrap());
    assert_eq!(load(&fixture.paths).view.zoom, 200);
    assert!(set_zoom(&fixture.paths, 0).unwrap());
    assert_eq!(load(&fixture.paths).view.zoom, 60);
    // Clamped to the value already saved is no change.
    assert!(!set_zoom(&fixture.paths, 10).unwrap());

    // A written default stays: only an absent key skips the write.
    assert!(set_zoom(&fixture.paths, 100).unwrap());
    assert!(fixture.read().contains("zoom = 100"), "{}", fixture.read());
    assert!(!set_zoom(&fixture.paths, 100).unwrap());
}

#[test]
fn set_zoom_replaces_a_bad_value_even_with_the_default() {
    let fixture = Fixture::new();
    for bad in ["59", "1.25", "\"125\"", "true", "-1"] {
        fixture.write(&format!("schema_version = 1\n\n[view]\nzoom = {bad}\n"));
        assert!(set_zoom(&fixture.paths, 100).unwrap(), "{bad}");
        assert_eq!(fixture.read(), "schema_version = 1\n\n[view]\nzoom = 100\n");
    }
    fixture.write("schema_version = 1\n\n[view]\nzoom = \"125\"\n");
    assert!(set_zoom(&fixture.paths, 125).unwrap());
    assert_eq!(load(&fixture.paths).view.zoom, 125);
}

#[test]
fn set_poster_only_writes_replaces_and_skips_no_ops() {
    let fixture = Fixture::new();
    assert!(!set_poster_only(&fixture.paths, false).unwrap());
    assert!(!fixture.paths.library_file().exists());

    assert!(set_poster_only(&fixture.paths, true).unwrap());
    assert!(load(&fixture.paths).view.poster_only);
    assert!(!set_poster_only(&fixture.paths, true).unwrap());

    // A written default stays.
    assert!(set_poster_only(&fixture.paths, false).unwrap());
    assert!(
        fixture.read().contains("poster_only = false"),
        "{}",
        fixture.read()
    );
    assert!(!set_poster_only(&fixture.paths, false).unwrap());

    fixture.write("schema_version = 1\n\n[view]\nposter_only = \"yes\"\n");
    assert!(set_poster_only(&fixture.paths, false).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n\n[view]\nposter_only = false\n"
    );
}

#[test]
fn zoom_and_poster_only_no_ops_leave_the_file_alone() {
    let fixture = Fixture::new();
    fixture
        .write("# keep formatting\nschema_version = 1\n\n[view]\nzoom = 125\nposter_only = true\n");
    let path = fixture.paths.library_file();
    let before = fs::metadata(&path).unwrap().ino();
    let bytes = fs::read(&path).unwrap();

    assert!(!set_zoom(&fixture.paths, 125).unwrap());
    assert!(!set_poster_only(&fixture.paths, true).unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(&path).unwrap().ino(), before);

    let absent = Fixture::new();
    absent.write("# keep formatting\nschema_version = 1\n\n[view]\nsort = \"name\"\n");
    let bytes = fs::read(absent.paths.library_file()).unwrap();
    assert!(!set_zoom(&absent.paths, 100).unwrap());
    assert!(!set_poster_only(&absent.paths, false).unwrap());
    assert_eq!(fs::read(absent.paths.library_file()).unwrap(), bytes);

    let missing = Fixture::new();
    assert!(!set_zoom(&missing.paths, 100).unwrap());
    assert!(!missing.paths.library_file().exists());
}

#[test]
fn zoom_and_poster_only_edits_keep_comments_and_other_keys() {
    let fixture = Fixture::new();
    fixture.write(
        "# my library\nschema_version = 1\n\n[view]\n# newest first\ndescending = true\nzoom = 90 # small\nfuture_view = \"kept\"\n\n[environments.hades]\n# my favorite\nfavorite = true\n",
    );

    assert!(set_zoom(&fixture.paths, 130).unwrap());
    assert!(set_poster_only(&fixture.paths, true).unwrap());
    let text = fixture.read();
    assert!(
        text.starts_with("# my library\nschema_version = 1\n"),
        "{text}"
    );
    assert!(text.contains("# newest first\ndescending = true"), "{text}");
    assert!(text.contains("zoom = 130 # small"), "{text}");
    assert!(text.contains("future_view = \"kept\""), "{text}");
    assert!(text.contains("poster_only = true"), "{text}");
    assert!(text.contains("# my favorite\nfavorite = true"), "{text}");
}

#[test]
fn zoom_and_poster_only_edits_leave_the_other_view_keys_alone() {
    let fixture = Fixture::new();
    assert!(set_order(&fixture.paths, SortKey::Playtime, true).unwrap());
    assert!(set_favorites_first(&fixture.paths, true).unwrap());
    assert!(set_zoom(&fixture.paths, 70).unwrap());
    assert!(set_poster_only(&fixture.paths, true).unwrap());
    assert_eq!(
        load(&fixture.paths).view,
        View {
            sort: SortKey::Playtime,
            descending: true,
            favorites_first: true,
            zoom: 70,
            poster_only: true,
        }
    );
}

#[test]
fn zoom_and_poster_only_edits_are_refused_on_a_broken_file() {
    let fixture = Fixture::new();
    let original = "schema_version = 1\n[view]\nzoom = 120\nthis is broken\n";
    fixture.write(original);
    assert!(set_zoom(&fixture.paths, 130).is_err());
    assert!(set_poster_only(&fixture.paths, true).is_err());
    assert_eq!(fixture.read(), original);
}
