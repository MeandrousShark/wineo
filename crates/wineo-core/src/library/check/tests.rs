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
fn the_source_and_tags_sorts_are_valid() {
    for sort in ["source", "tags"] {
        let inspection = inspect_text(&format!("schema_version = 1\n[view]\nsort = \"{sort}\"\n"));
        assert_eq!(inspection.findings, [], "{sort}");
    }
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
            "library.toml: view.sort: expected one of name, last-played, playtime, added, source, tags, found \"random\"",
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
    let inspection = inspect_text(
        "zoom = 2\n[view]\nfuture_view = \"grid\"\n[environments.hades]\nnotes = \"hi\"\n",
    );
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        [
            "library.toml: zoom: unknown key, ignored",
            "library.toml: view.future_view: unknown key, ignored",
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

#[test]
fn zoom_and_poster_only_are_known_view_keys() {
    let inspection = inspect_text("schema_version = 1\n[view]\nzoom = 60\nposter_only = true\n");
    assert_eq!(inspection.findings, []);
    let inspection = inspect_text("[view]\nzoom = 200\nposter_only = false\n");
    assert_eq!(inspection.findings, []);
    let inspection = inspect_text("[view]\nzoom = 125\nzoom_steps = 3\n");
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        ["library.toml: view.zoom_steps: unknown key, ignored"]
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [] as [String; 0]
    );
}

#[test]
fn bad_zoom_and_poster_only_values_are_reported_like_the_other_view_keys() {
    let inspection = inspect_text(
        r#"[view]
zoom = 59
poster_only = "yes"
"#,
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: view.zoom: expected a whole number from 60 to 200, found 59",
            "library.toml: view.poster_only: expected true or false, found \"yes\"",
        ]
    );
    for bad in ["201", "0", "-5", "1.25", "\"125\"", "true"] {
        let inspection = inspect_text(&format!("[view]\nzoom = {bad}\n"));
        assert_eq!(
            messages(&inspection, FindingKind::FieldInvalid).len(),
            1,
            "{bad}"
        );
        assert_eq!(
            messages(&inspection, FindingKind::KeyUnknown).len(),
            0,
            "{bad}"
        );
    }
}

#[test]
fn random_is_a_known_view_key_and_a_bad_value_is_reported() {
    for good in ["launchable", "any"] {
        let inspection = inspect_text(&format!("[view]\nrandom = \"{good}\"\n"));
        assert_eq!(inspection.findings, [], "{good}");
    }
    let inspection = inspect_text("[view]\nrandom = \"favorites\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: view.random: expected one of launchable, any, found \"favorites\""]
    );
    assert_eq!(messages(&inspection, FindingKind::KeyUnknown).len(), 0);
    let inspection = inspect_text("[view]\nrandom = true\n");
    assert_eq!(messages(&inspection, FindingKind::FieldInvalid).len(), 1);
}

#[test]
fn layout_is_a_known_view_key_and_a_bad_value_is_reported() {
    for good in ["grid", "list"] {
        let inspection = inspect_text(&format!("[view]\nlayout = \"{good}\"\n"));
        assert_eq!(inspection.findings, [], "{good}");
    }
    let inspection = inspect_text("[view]\nlayout = \"table\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: view.layout: expected one of grid, list, found \"table\""]
    );
    assert_eq!(messages(&inspection, FindingKind::KeyUnknown).len(), 0);
    let inspection = inspect_text("[view]\nlayout = true\n");
    assert_eq!(messages(&inspection, FindingKind::FieldInvalid).len(), 1);
}

#[test]
fn group_is_a_known_view_key_and_a_bad_value_is_reported() {
    for good in ["none", "recent", "tag", "source", "state"] {
        let inspection = inspect_text(&format!("[view]\ngroup = \"{good}\"\n"));
        assert_eq!(inspection.findings, [], "{good}");
    }
    let inspection = inspect_text("[view]\ngroup = \"genre\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: view.group: expected one of none, recent, tag, source, state, found \"genre\""
        ]
    );
    assert_eq!(messages(&inspection, FindingKind::KeyUnknown).len(), 0);
    let inspection = inspect_text("[view]\ngroup = true\n");
    assert_eq!(messages(&inspection, FindingKind::FieldInvalid).len(), 1);
}

fn inspect_with_themes(text: &str, themes: &[(&str, &str)]) -> Inspection {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), text).unwrap();
    fs::create_dir_all(paths.themes_dir()).unwrap();
    for (name, text) in themes {
        fs::write(paths.themes_dir().join(name), text).unwrap();
    }
    inspect(&paths)
}

#[test]
fn a_clean_appearance_has_no_findings() {
    let inspection = inspect_with_themes(
        "[appearance]\nfollow_system = false\ndark = \"dracula\"\nlight = \"custom:soft\"\ntheme = \"custom:night\"\nimprove_readability = true\nmotion = \"reduced\"\ninterface_size = 130\nfont = \"Atkinson Hyperlegible\"\n",
        &[
            ("soft.toml", "name = \"Soft\"\nkind = \"light\"\n"),
            ("night.toml", "name = \"Night\"\n"),
        ],
    );
    assert_eq!(inspection.findings, []);
}

#[test]
fn bad_appearance_types_are_field_findings_and_unknown_keys_are_ignored() {
    let inspection = inspect_text(
        "[appearance]\nfollow_system = \"yes\"\ndark = 3\nimprove_readability = 1\nextra = true\n",
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: appearance.follow_system: expected true or false, found \"yes\"",
            "library.toml: appearance.dark: expected a theme id, found 3",
            "library.toml: appearance.improve_readability: expected true or false, found 1",
        ]
    );
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        ["library.toml: appearance.extra: unknown key, ignored"]
    );
    let inspection = inspect_text("appearance = 4\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: appearance: expected a table, found 4"]
    );
}

#[test]
fn an_unknown_or_mistyped_motion_is_a_field_finding() {
    for motion in ["system", "full", "reduced"] {
        let inspection = inspect_text(&format!("[appearance]\nmotion = \"{motion}\"\n"));
        assert_eq!(inspection.findings, [], "{motion}");
    }
    let inspection = inspect_text("[appearance]\nmotion = \"slow\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: appearance.motion: expected one of system, full, reduced, found \"slow\""]
    );
    let inspection = inspect_text("[appearance]\nmotion = false\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: appearance.motion: expected one of system, full, reduced, found false"]
    );
}

#[test]
fn an_interface_size_outside_the_range_or_mistyped_is_a_field_finding() {
    for size in ["100", "130", "200"] {
        let inspection = inspect_text(&format!("[appearance]\ninterface_size = {size}\n"));
        assert_eq!(inspection.findings, [], "{size}");
    }
    for (size, found) in [("250", "250"), ("99", "99"), ("\"150\"", "\"150\"")] {
        let inspection = inspect_text(&format!("[appearance]\ninterface_size = {size}\n"));
        assert_eq!(
            messages(&inspection, FindingKind::FieldInvalid),
            [format!(
                "library.toml: appearance.interface_size: expected a whole number from 100 to 200, found {found}"
            )]
        );
    }
}

#[test]
fn a_font_that_is_not_a_name_is_a_field_finding() {
    // Whether the family is installed only shows at the app's launch.
    let inspection = inspect_text("[appearance]\nfont = \"Not Installed Anywhere\"\n");
    assert_eq!(inspection.findings, []);
    for (font, found) in [("\"\"", "\"\""), ("3", "3")] {
        let inspection = inspect_text(&format!("[appearance]\nfont = {font}\n"));
        assert_eq!(
            messages(&inspection, FindingKind::FieldInvalid),
            [format!(
                "library.toml: appearance.font: expected a font family name, found {found}"
            )]
        );
    }
}

#[test]
fn theme_ids_that_cannot_be_used_are_field_findings() {
    let inspection = inspect_with_themes(
        "[appearance]\ndark = \"nope\"\nlight = \"custom:gone\"\ntheme = \"custom:broken\"\n",
        &[("broken.toml", "name = \n")],
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: appearance.dark: expected a built-in theme or custom:<id> of a theme in themes/, found \"nope\"",
            "library.toml: appearance.light: expected a theme file in themes/ (gone.toml is missing), found \"custom:gone\"",
            "library.toml: appearance.theme: expected a theme file that can be read (broken.toml in themes/ can't), found \"custom:broken\"",
        ]
    );
}

#[test]
fn a_theme_of_the_wrong_kind_in_dark_or_light_is_a_finding() {
    let inspection = inspect_with_themes(
        "[appearance]\ndark = \"daylight\"\nlight = \"custom:night\"\ntheme = \"daylight\"\n",
        &[("night.toml", "kind = \"dark\"\n")],
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: appearance.dark: expected a dark theme, found \"daylight\"",
            "library.toml: appearance.light: expected a light theme, found \"custom:night\"",
        ]
    );
}

#[test]
fn a_custom_theme_without_a_usable_kind_counts_as_dark() {
    let inspection = inspect_with_themes(
        "[appearance]\ndark = \"custom:odd\"\nlight = \"custom:odd\"\n",
        &[("odd.toml", "kind = \"sepia\"\n")],
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: appearance.light: expected a light theme, found \"custom:odd\""]
    );
}

#[test]
fn a_clean_preset_has_no_findings() {
    let inspection = inspect_text(
        r#"schema_version = 1

[presets."Couch co-op"]
sort = "last-played"
descending = true
favorites_first = false
stores = ["steam", "gog"]
platforms = ["native"]
tags = ["co-op", "any text at all"]
not_stores = ["epic", "amazon"]
not_platforms = ["windows"]
not_tags = ["vr"]
show_hidden = false
group = "tag"
layout = "list"

[presets.Empty]
"#,
    );
    assert_eq!(inspection.unreadable, None);
    assert_eq!(inspection.findings, []);
    assert_eq!(inspection.entries, Vec::<String>::new());
}

#[test]
fn a_preset_may_group_by_recently_played() {
    let inspection = inspect_text("[presets.Recents]\ngroup = \"recent\"\n");
    assert_eq!(inspection.findings, []);
}

#[test]
fn bad_preset_values_name_the_preset_the_field_and_what_was_found() {
    let inspection = inspect_text(
        r#"[presets."Couch co-op"]
sort = "random"
descending = "yes"
favorites_first = 1
show_hidden = "no"
group = "genre"
layout = true
stores = "steam"
platforms = ["macos", "native", 3]
not_stores = ["Steam"]
not_platforms = [["windows"]]
tags = ["ok", 3]
not_tags = 5
"#,
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            r#"library.toml: presets."Couch co-op".sort: expected one of name, last-played, playtime, added, source, tags, found "random""#,
            r#"library.toml: presets."Couch co-op".descending: expected true or false, found "yes""#,
            r#"library.toml: presets."Couch co-op".favorites_first: expected true or false, found 1"#,
            r#"library.toml: presets."Couch co-op".show_hidden: expected true or false, found "no""#,
            r#"library.toml: presets."Couch co-op".group: expected one of none, recent, tag, source, state, found "genre""#,
            r#"library.toml: presets."Couch co-op".layout: expected one of grid, list, found true"#,
            r#"library.toml: presets."Couch co-op".stores: expected a list of store ids: steam, gog, epic, amazon, found "steam""#,
            r#"library.toml: presets."Couch co-op".platforms: expected a list of platform ids: native, windows, found "macos""#,
            r#"library.toml: presets."Couch co-op".platforms: expected a list of platform ids: native, windows, found 3"#,
            r#"library.toml: presets."Couch co-op".not_stores: expected a list of store ids: steam, gog, epic, amazon, found "Steam""#,
            r#"library.toml: presets."Couch co-op".not_platforms: expected a list of platform ids: native, windows, found ["windows"]"#,
            r#"library.toml: presets."Couch co-op".tags: expected a list of tag names, found 3"#,
            r#"library.toml: presets."Couch co-op".not_tags: expected a list of tag names, found 5"#,
        ]
    );
    assert!(inspection.findings.iter().all(|f| f.environment.is_none()));
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        [] as [String; 0]
    );
}

#[test]
fn a_preset_with_one_bad_field_still_reports_only_that_field() {
    let inspection =
        inspect_text("[presets.P]\nsort = \"added\"\nstores = [\"steam\", \"itch\"]\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            r#"library.toml: presets.P.stores: expected a list of store ids: steam, gog, epic, amazon, found "itch""#
        ]
    );
}

#[test]
fn unknown_preset_keys_are_warned_about_and_ignored() {
    let inspection =
        inspect_text("[presets.P]\nsort = \"added\"\nfuture = 1\nsearch = \"hades\"\nzoom = 100\n");
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        [
            "library.toml: presets.P.future: unknown key, ignored",
            "library.toml: presets.P.search: unknown key, ignored",
            "library.toml: presets.P.zoom: unknown key, ignored",
        ]
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [] as [String; 0]
    );
}

#[test]
fn presets_of_the_wrong_shape_are_bad_values() {
    let inspection = inspect_text("presets = 1\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: presets: expected a table, found 1"]
    );

    let inspection = inspect_text(
        "[presets]\nbroken = \"x\"\n[presets.\"\"]\nsort = \"added\"\n[presets.\" \"]\n[presets.Good]\nsort = \"added\"\n",
    );
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            r#"library.toml: presets.broken: expected a table, found "x""#,
            r#"library.toml: presets."": expected a preset name that isn't empty, found a table"#,
            r#"library.toml: presets." ": expected a preset name that isn't empty, found a table"#,
        ]
    );
    assert_eq!(inspection.entries, Vec::<String>::new());
}

#[test]
fn presets_name_no_environment_so_they_are_never_strays() {
    let inspection = inspect_text("[presets.hades]\nsort = \"added\"\n");
    assert_eq!(inspection.entries, Vec::<String>::new());
    assert_eq!(inspection.findings, []);
}

#[test]
fn preset_names_the_write_api_would_refuse_are_findings() {
    let long = "x".repeat(61);
    let fine = "y".repeat(60);
    let text = format!(
        "[presets.\" lead\"]\nsort = \"added\"\n[presets.\"trail \"]\n[presets.\"a\\tb\"]\n[presets.\"{long}\"]\n[presets.\"{fine}\"]\n[presets.\"Couch co-op\"]\n"
    );
    let inspection = inspect_text(&text);
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            r#"library.toml: presets." lead": expected a preset name that can't start or end with whitespace, found a table"#.to_owned(),
            r#"library.toml: presets."trail ": expected a preset name that can't start or end with whitespace, found a table"#.to_owned(),
            r#"library.toml: presets."a\tb": expected a preset name that can't contain control characters, found a table"#.to_owned(),
            format!(
                r#"library.toml: presets.{long}: expected a preset name that can't be longer than 60 characters, found a table"#
            ),
        ]
    );

    // The reader keeps showing every one of them.
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    fs::create_dir_all(paths.library_file().parent().unwrap()).unwrap();
    fs::write(paths.library_file(), &text).unwrap();
    let names: Vec<String> = crate::library::load(&paths)
        .presets
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names.len(), 6, "{names:?}");
}

#[test]
fn a_badly_named_preset_still_has_its_fields_checked() {
    let inspection = inspect_text("[presets.\" P\"]\nsort = \"random\"\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            r#"library.toml: presets." P": expected a preset name that can't start or end with whitespace, found a table"#,
            r#"library.toml: presets." P".sort: expected one of name, last-played, playtime, added, source, tags, found "random""#,
        ]
    );
}

#[test]
fn a_clean_desktop_has_no_findings() {
    for close in ["quit", "tray"] {
        let inspection = inspect_text(&format!("[desktop]\ntray = false\nclose = \"{close}\"\n"));
        assert_eq!(inspection.findings, [], "{close}");
    }
}

#[test]
fn bad_desktop_values_are_field_findings_and_unknown_keys_are_reported() {
    let inspection =
        inspect_text("[desktop]\ntray = \"yes\"\nclose = \"minimize\"\nextra = true\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        [
            "library.toml: desktop.tray: expected true or false, found \"yes\"",
            "library.toml: desktop.close: expected one of quit, tray, found \"minimize\"",
        ]
    );
    assert_eq!(
        messages(&inspection, FindingKind::KeyUnknown),
        ["library.toml: desktop.extra: unknown key, ignored"]
    );
    let inspection = inspect_text("[desktop]\nclose = 1\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: desktop.close: expected one of quit, tray, found 1"]
    );
    let inspection = inspect_text("desktop = 4\n");
    assert_eq!(
        messages(&inspection, FindingKind::FieldInvalid),
        ["library.toml: desktop: expected a table, found 4"]
    );
}
