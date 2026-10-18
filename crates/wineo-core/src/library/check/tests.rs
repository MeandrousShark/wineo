use std::ffi::OsString;

use super::*;

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().join("home");
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn inspect_text(text: &str) -> Inspection {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
    inspect(&paths)
}

fn messages(inspection: &Inspection, kind: FindingKind) -> Vec<String> {
    inspection
        .findings
        .iter()
        .filter(|finding| finding.kind == kind)
        .map(|finding| finding.message.clone())
        .collect()
}

#[test]
fn a_missing_or_clean_file_has_no_findings() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(inspect(&paths(&root)), Inspection::default());

    let inspection = inspect_text(
        "schema_version = 1\n[view]\nsort = \"added\"\ndescending = true\nfavorites_first = false\n[environments.hades]\nfavorite = true\nhidden = false\nadded = 2026-10-17T18:00:00Z\n",
    );
    assert_eq!(inspection.unreadable, None);
    assert_eq!(inspection.findings, []);
    assert_eq!(inspection.entries, ["hades"]);
}

#[test]
fn bad_values_name_the_field_what_was_expected_and_what_was_found() {
    let inspection = inspect_text(
        r#"[view]
sort = "random"
descending = "yes"

[environments.hades]
favorite = "yes"
added = 2026-10-17
hidden = true

[environments."my game"]
hidden = 1
"#,
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: view.sort: expected one of name, last-played, playtime, added, source, found \"random\"",
            "library.toml: view.descending: expected true or false, found \"yes\"",
            "library.toml: environments.hades.favorite: expected true or false, found \"yes\"",
            "library.toml: environments.hades.added: expected a date and time with an offset, like 2026-10-17T18:00:00Z, found 2026-10-17",
            "library.toml: environments.\"my game\".hidden: expected true or false, found 1",
        ]
    );
    let environments: Vec<Option<&str>> = inspection
        .findings
        .iter()
        .map(|finding| finding.environment.as_deref())
        .collect();
    assert_eq!(
        environments,
        [None, None, Some("hades"), Some("hades"), Some("my game")]
    );
}

#[test]
fn tables_of_the_wrong_shape_are_bad_values() {
    let inspection = inspect_text("view = 1\n[environments]\nhades = true\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: view: expected a table, found 1",
            "library.toml: environments.hades: expected a table, found true",
        ]
    );
    assert_eq!(inspection.entries, ["hades"]);
}

#[test]
fn unknown_keys_are_found_at_every_level() {
    let inspection =
        inspect_text("zoom = 2\n[view]\nlayout = \"grid\"\n[environments.hades]\nnotes = \"hi\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        [
            "library.toml: zoom: unknown key, ignored",
            "library.toml: view.layout: unknown key, ignored",
            "library.toml: environments.hades.notes: unknown key, ignored",
        ]
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        Vec::<String>::new()
    );
}

#[test]
fn a_syntax_error_names_its_line() {
    let inspection = inspect_text("schema_version = 1\n\n[environments.hades]\nfavorite true\n");
    let unreadable = inspection.unreadable.unwrap();
    assert!(!unreadable.newer);
    assert!(
        unreadable
            .message
            .starts_with("library.toml can't be read (line 4: "),
        "{}",
        unreadable.message
    );
    assert_eq!(inspection.findings, []);
}

#[test]
fn schema_versions_tell_newer_from_unsupported() {
    let newer = inspect_text("schema_version = 2\n").unreadable.unwrap();
    assert!(newer.newer);
    assert_eq!(
        newer.message,
        "library.toml was written by a newer wineo; update wineo to use it."
    );
    for (version, shown) in [("0", "0"), ("\"2\"", "\"2\"")] {
        let old = inspect_text(&format!("schema_version = {version}\n"))
            .unreadable
            .unwrap();
        assert!(!old.newer, "{version}");
        assert_eq!(
            old.message,
            format!("library.toml can't be read (unsupported schema_version {shown})")
        );
    }
}

#[test]
fn a_newer_schema_version_is_found_in_a_file_that_does_not_parse() {
    let inspection =
        inspect_text("schema_version = 2\n[environments.hades]\nfavorite = true\nbroken =\n");
    let unreadable = inspection.unreadable.unwrap();
    assert!(unreadable.newer);
    assert_eq!(
        unreadable.message,
        "library.toml was written by a newer wineo; update wineo to use it."
    );

    // Only a top-level one counts, and not one inside a multi-line value.
    for text in [
        "[environments.hades]\nschema_version = 2\nbroken =\n",
        "note = \"\"\"\nschema_version = 2\n\"\"\"\nbroken =\n",
    ] {
        assert!(!inspect_text(text).unreadable.unwrap().newer, "{text}");
    }
}
