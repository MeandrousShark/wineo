use std::ffi::OsString;

use super::*;
use crate::files::sync_dir;
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

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

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

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

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

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

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

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (1, 1, 0));
    assert!(report.dropped_text);
}

#[test]
fn a_failed_backup_aborts_and_leaves_the_original_untouched() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    // A stamp naming a missing directory makes only the backup fail, so the
    // file would otherwise be writable.
    let error = repair_at(&fixture.paths, "missing/dir", sync_dir).unwrap_err();

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

    let result = repair_at(&fixture.paths, STAMP, sync_dir);

    fs::set_permissions(fixture.dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fixture.read(), DAMAGED);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn two_repairs_in_the_same_second_keep_both_backups() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);
    let first = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();
    let second_original = "[environments.celeste]\nhidden = true\n[environments.celeste]\n";
    fixture.write(second_original);

    let second = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

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
    assert_eq!(repair_at(&fixture.paths, STAMP, sync_dir).unwrap(), None);
    assert!(!fixture.paths.library_file().exists());

    let readable = "schema_version = 1\n\n[environments.hades]\nfavorite = \"yes\"\n";
    fixture.write(readable);
    assert_eq!(repair_at(&fixture.paths, STAMP, sync_dir).unwrap(), None);
    assert_eq!(fixture.read(), readable);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn a_newer_schema_version_is_refused() {
    let fixture = Fixture::new();
    let newer = "schema_version = 2\n\n[environments.hades]\nfavorite = true\n";
    fixture.write(newer);

    let error = repair_at(&fixture.paths, STAMP, sync_dir).unwrap_err();

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

        let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

        assert!(report.complete(), "{report:?}");
        assert!(fixture.read().starts_with("schema_version = 1\n"));
        assert!(load(&fixture.paths).environments["hades"].favorite);
    }
}

#[test]
fn a_line_that_is_not_utf8_is_lost_and_reported() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.dir()).unwrap();
    let bytes = b"[environments.hades]\nnote = \"\xff\"\nfavorite = true\n".to_vec();
    fs::write(fixture.paths.library_file(), &bytes).unwrap();

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (1, 0, 1));
    assert!(!report.complete());
    assert_eq!(fs::read(&report.backup).unwrap(), bytes);
    assert!(load(&fixture.paths).environments["hades"].favorite);
}

#[test]
fn a_leading_byte_order_mark_is_skipped() {
    let fixture = Fixture::new();
    fixture.write("\u{FEFF}[environments.hades]\nfavorite = true\nbroken =\n");

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.partly), (1, 1));
    assert!(!report.dropped_text);
    assert!(load(&fixture.paths).environments["hades"].favorite);
}

#[test]
fn a_multi_line_value_is_one_field_and_hides_the_headers_inside_it() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nnote = \"\"\"\n[environments.celeste]\nfavorite = true\n\"\"\"\nhidden = true\nbroken =\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (1, 0, 1));
    assert!(!report.dropped_text);
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n\n[environments.hades]\nnote = \"\"\"\n[environments.celeste]\nfavorite = true\n\"\"\"\nhidden = true\n"
    );
    let library = load(&fixture.paths);
    assert!(library.environments["hades"].hidden);
    assert!(!library.environments.contains_key("celeste"));
}

#[test]
fn multi_line_arrays_and_literal_strings_are_kept_whole() {
    let fixture = Fixture::new();
    let text = "[environments.hades]\ntags = [\n  \"roguelike\", # best\n  \"]\",\n]\nnote = \'\'\'\nfavorite = false\n\'\'\'\nfavorite = true\nbroken =\n";
    fixture.write(text);

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.partly), (1, 1));
    let library = fs::read_to_string(fixture.paths.library_file()).unwrap();
    assert!(
        library.contains("tags = [\n  \"roguelike\", # best\n  \"]\",\n]\n"),
        "{library}"
    );
    assert!(load(&fixture.paths).environments["hades"].favorite);
}

#[test]
fn an_unterminated_multi_line_value_swallows_the_rest_as_lost() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nfavorite = true\nnote = \"\"\"\n[environments.celeste]\nfavorite = true\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (1, 0, 1));
    let library = load(&fixture.paths);
    assert!(library.environments["hades"].favorite);
    assert!(!library.environments.contains_key("celeste"));
}

#[test]
fn dotted_keys_are_compared_by_their_full_path() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nfuture.a = 1\nfuture.b = 2\nfuture = 3\nfavorite = true\nfavorite = false\nbroken =\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.partly), (1, 1));
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n\n[environments.hades]\nfuture.a = 1\nfuture.b = 2\nfavorite = true\n"
    );
    assert_eq!(load(&fixture.paths).problem, None);
}

#[test]
fn a_newer_schema_version_in_a_file_that_does_not_parse_is_refused() {
    let fixture = Fixture::new();
    let newer = "schema_version = 2\n[environments.hades]\nfavorite = true\nbroken =\n";
    fixture.write(newer);

    let error = repair_at(&fixture.paths, STAMP, sync_dir).unwrap_err();

    assert!(matches!(error, Error::SchemaVersion { .. }), "{error}");
    assert_eq!(fixture.read(), newer);
    assert_eq!(fixture.files(), ["library.toml"]);
}

#[test]
fn the_backup_is_durable_before_the_original_is_replaced() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    let report = repair_at(&fixture.paths, STAMP, |dir| {
        assert_eq!(dir, fixture.dir());
        assert_eq!(fixture.read(), DAMAGED);
        assert_eq!(fs::read_to_string(fixture.backup(STAMP)).unwrap(), DAMAGED);
        sync_dir(dir)
    })
    .unwrap()
    .unwrap();
    assert_eq!(report.backup, fixture.backup(STAMP));
    assert_ne!(fixture.read(), DAMAGED);
}

#[test]
fn a_failed_directory_sync_leaves_the_original_and_keeps_the_backup() {
    let fixture = Fixture::new();
    fixture.write(DAMAGED);

    let error = repair_at(&fixture.paths, STAMP, |_| {
        Err(io::Error::other("sync failed"))
    })
    .unwrap_err();

    assert!(error.to_string().contains("failed to sync"), "{error}");
    assert_eq!(fixture.read(), DAMAGED);
    assert_eq!(fs::read_to_string(fixture.backup(STAMP)).unwrap(), DAMAGED);
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

#[test]
#[ignore = "timing, run with --release --ignored --nocapture"]
fn bench_repair_of_two_thousand_fields() {
    let mut text = String::new();
    for index in 0..1000 {
        text.push_str(&format!(
            "[environments.game{index}]\nfavorite = true\nadded = 2026-10-17T18:00:00Z\n\n"
        ));
    }
    let mut one_entry = String::from("[environments.big]\n");
    for index in 0..2000 {
        one_entry.push_str(&format!("field{index} = {index}\n"));
    }
    for (label, broken) in [
        (
            "one entry, broken at the end",
            format!("{one_entry}broken =\n"),
        ),
        (
            "one entry, broken at the start",
            format!("broken =\n{one_entry}"),
        ),
        ("broken at the end", format!("{text}broken =\n")),
        ("broken at the start", format!("broken =\n{text}")),
        (
            "unterminated string at the start",
            format!("[environments.x]\nnote = \"\"\"\n{text}"),
        ),
        (
            "unterminated string in the middle",
            format!("{text}note = '''\n{text}"),
        ),
    ] {
        let fixture = Fixture::new();
        fixture.write(&broken);
        let started = std::time::Instant::now();
        repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();
        eprintln!("{label}: {:?}", started.elapsed());
    }
}

/// Every valid field toml_edit reads from `text`, flattened to
/// `section/dotted.path -> value`, with string newlines normalized.
fn attribution(text: &str) -> std::collections::BTreeMap<String, String> {
    fn canonical(value: &toml_edit::Value) -> String {
        match value {
            toml_edit::Value::String(text) => format!("{:?}", text.value().replace("\r\n", "\n")),
            toml_edit::Value::Array(array) => format!(
                "[{}]",
                array.iter().map(canonical).collect::<Vec<_>>().join(",")
            ),
            toml_edit::Value::InlineTable(table) => format!(
                "{{{}}}",
                table
                    .iter()
                    .map(|(key, value)| format!("{key}={}", canonical(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            other => {
                let mut other = other.clone();
                other.decor_mut().clear();
                other.to_string()
            }
        }
    }
    fn flatten(path: String, item: &Item, out: &mut std::collections::BTreeMap<String, String>) {
        match item {
            Item::Value(value) => {
                out.insert(path, canonical(value));
            }
            Item::Table(table) => {
                for (key, child) in table.iter() {
                    flatten(format!("{path}.{key}"), child, out);
                }
            }
            Item::ArrayOfTables(array) => {
                for (index, table) in array.iter().enumerate() {
                    for (key, child) in table.iter() {
                        flatten(format!("{path}[{index}].{key}"), child, out);
                    }
                }
            }
            Item::None => {}
        }
    }
    let document = text.parse::<DocumentMut>().expect("a valid document");
    let mut out = std::collections::BTreeMap::new();
    let mut section =
        |name: &str, table: &dyn toml_edit::TableLike, valid: fn(&str, &Item) -> bool| {
            for (key, item) in table.iter() {
                if valid(key, item) {
                    flatten(format!("{name}/{key}"), item, &mut out);
                }
            }
        };
    if let Some(view) = document.get("view").and_then(Item::as_table_like) {
        section("[view]", view, valid_view_field);
    }
    if let Some(environments) = document.get("environments").and_then(Item::as_table_like) {
        for (name, entry) in environments.iter() {
            section(name, entry.as_table_like().unwrap(), valid_entry_field);
        }
    }
    out
}

/// Valid documents full of constructs a line scanner could misread. Each
/// is (name, text, the table its last line belongs to).
const NASTY: &[(&str, &str, &str)] = &[
    (
        "quote runs closing basic and literal strings",
        "[environments.hades]\nnote = \"\"\"ends with one quote\"\"\"\"\nother = \"\"\"ends with two\"\"\"\"\"\nlit = '''one''''\nlit2 = '''two'''''\nempty = \"\"\"\"\"\"\nfavorite = true\n",
        "hades",
    ),
    (
        "round two: a quote run inside a multi-line array",
        "[environments.hades]\nnote = [\"\"\"one\"\"\"\", \"\"\"\n]\n[environments.celeste]\nfavorite = true\n\"\"\"]\nhidden = true\n",
        "hades",
    ),
    (
        "round two with literal strings",
        "[environments.hades]\nnote = ['''one'''', '''\n]\n[environments.celeste]\nfavorite = true\n''']\nhidden = true\n",
        "hades",
    ),
    (
        "round one: a header inside a multi-line string",
        "[environments.hades]\nnote = \"\"\"\n[environments.celeste]\nfavorite = true\n\"\"\"\nhidden = true\n",
        "hades",
    ),
    (
        "literal strings holding headers and triple quotes",
        "[environments.celeste]\nnote = '[environments.x]'\nnote2 = '''\n[environments.fake]\nfavorite = true\n\"\"\"\n'''\nescaped = \"\"\"a \\\"\"\" b\"\"\"\nhidden = true\n",
        "celeste",
    ),
    (
        "comments and brackets inside strings in arrays",
        "[environments.\"old demo\"]\ntags = [\"#not a comment\", \"]\", \"[\", '''\n]''', # a real comment\n  \"[environments.ghost]\",\n]\nfavorite = true # real comment\n",
        "old demo",
    ),
    (
        "header-looking lines in multi-line arrays and inline tables",
        "[view]\nsort = \"added\"\nextra = [\n  \"[environments.ghost]\",\n  { a = \"[view]\" },\n[\n\"x\"]\n]\ndescending = true\n[environments.last]\nhidden = true\n",
        "last",
    ),
    (
        "dotted keys",
        "[environments.dotted]\nfuture.a = 1\nfuture.b = \"x\"\nsite.\"google.com\" = true\nfavorite = true\n",
        "dotted",
    ),
    (
        "CRLF line endings",
        "[environments.windows]\r\nnote = \"\"\"\r\n[environments.ghost]\r\n\"\"\"\r\nfavorite = true\r\n\r\n[environments.second]\r\nhidden = true\r\n",
        "second",
    ),
];

fn assert_repairs_like_the_parser(original: &str, valid: &str, broken_in: &str, case: &str) {
    let fixture = Fixture::new();
    fixture.write(original);
    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    let expected = attribution(valid);
    assert_eq!(attribution(&fixture.read()), expected, "{case}");
    let document = valid.parse::<DocumentMut>().unwrap();
    let entries = document
        .get("environments")
        .and_then(Item::as_table_like)
        .map_or(0, |table| table.len());
    let partly = usize::from(broken_in != "[view]");
    assert_eq!(
        (report.entries, report.full, report.partly),
        (entries, entries - partly, partly),
        "{case}"
    );
    let view = match (document.get("view"), broken_in) {
        (None, _) => ViewRecovery::Absent,
        (Some(_), "[view]") => ViewRecovery::Partly,
        (Some(_), _) => ViewRecovery::Recovered,
    };
    assert_eq!(report.view, view, "{case}");
    assert!(!report.dropped_text, "{case}");
}

#[test]
fn a_broken_last_line_leaves_everything_before_it_as_the_parser_reads_it() {
    for (case, valid, last) in NASTY {
        valid
            .parse::<DocumentMut>()
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        let original = format!("{valid}broken =\n");
        assert_repairs_like_the_parser(&original, valid, last, case);
    }
}

#[test]
fn a_broken_line_in_the_middle_leaves_the_tail_attributed_too() {
    for (case, nasty, _) in NASTY {
        let head = "[environments.first]\nfavorite = true\n";
        let original = format!("{head}broken =\n{nasty}");
        let valid = format!("{head}{nasty}");
        assert_repairs_like_the_parser(&original, &valid, "first", case);
    }
}

#[test]
fn the_tail_continues_the_table_the_parsed_part_ended_in() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nfavorite = true\nbroken =\nhidden = true\n[environments.celeste]\nhidden = true\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (2, 1, 1));
    let library = load(&fixture.paths);
    assert!(library.environments["hades"].favorite);
    assert!(library.environments["hades"].hidden);
    assert!(library.environments["celeste"].hidden);
}

#[test]
fn an_entry_split_across_the_parsed_part_and_the_tail_counts_once() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nfavorite = true\nfuture.a = 1\n[view]\nbroken =\n[environments.hades]\nhidden = true\nfavorite = false\nfuture.b = 2\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    // The second `favorite` is a duplicate, so hades is partly recovered.
    assert_eq!((report.entries, report.full, report.partly), (1, 0, 1));
    assert_eq!(report.view, ViewRecovery::Lost);
    let library = load(&fixture.paths);
    assert!(library.environments["hades"].favorite);
    assert!(library.environments["hades"].hidden);
    let text = fixture.read();
    assert!(
        text.contains("future.a = 1") && text.contains("future.b = 2"),
        "{text}"
    );
}

#[test]
fn an_unterminated_string_falls_back_to_the_last_prefix_that_parses() {
    let fixture = Fixture::new();
    fixture.write(
        "[environments.hades]\nfavorite = true\n[environments.celeste]\nnote = '''\n[environments.ghost]\nfavorite = true\n",
    );

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!((report.entries, report.full, report.partly), (2, 1, 0));
    let library = load(&fixture.paths);
    assert!(library.environments["hades"].favorite);
    assert!(library.environments.contains_key("celeste"));
    assert!(!library.environments.contains_key("ghost"));
}

#[test]
fn a_file_with_nothing_that_parses_is_scanned_from_the_start() {
    let fixture = Fixture::new();
    fixture.write("note = \"\"\"\n[environments.ghost]\nfavorite = true\n");

    let report = repair_at(&fixture.paths, STAMP, sync_dir).unwrap().unwrap();

    assert_eq!(report.entries, 0);
    assert!(report.dropped_text);
}

#[test]
fn quote_runs_close_multi_line_strings_as_the_spec_says() {
    let scan = |lines: &[&str]| {
        let mut scan = Scan::default();
        for (index, line) in lines.iter().enumerate() {
            scan.line(line, index == 0);
        }
        scan.open()
    };
    // Three to five quotes close, the extra ones being content.
    for close in ["\"\"\"", "\"\"\"\"", "\"\"\"\"\""] {
        assert!(!scan(&[&format!("a = \"\"\"x{close}")]), "{close}");
        assert!(!scan(&["a = \"\"\"", &format!("x{close}")]), "{close}");
    }
    for close in ["'''", "''''", "'''''"] {
        assert!(!scan(&[&format!("a = '''x{close}")]), "{close}");
    }
    // One or two quotes are content.
    assert!(scan(&["a = \"\"\"x\"\"", "y\""]));
    assert!(scan(&["a = '''x''", "y'"]));
    // Escapes only count in basic strings.
    assert!(scan(&["a = \"\"\"x\\\"\"\""]));
    assert!(!scan(&["a = '''x\\'''"]));
    // A run right after the opening delimiter closes it too.
    assert!(!scan(&["a = \"\"\"\"\"\""]));
    // A run of four inside an array closes the string, not the array.
    assert!(scan(&["a = [\"\"\"one\"\"\"\", \"\"\""]));
    assert!(!scan(&["a = [\"\"\"one\"\"\"\", \"\"\"", "]", "\"\"\"]"]));
    // A comment or a quote inside a string doesn't end the value.
    assert!(!scan(&["a = [\"#\", \"]\"]"]));
    assert!(scan(&["a = [\"#\", \"]\","]));
}
