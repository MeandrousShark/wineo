use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;

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

    fn file(&self, id: &str) -> PathBuf {
        self.paths.themes_dir().join(format!("{id}.toml"))
    }

    fn write(&self, id: &str, text: &str) {
        fs::create_dir_all(self.paths.themes_dir()).unwrap();
        fs::write(self.file(id), text).unwrap();
    }

    fn read(&self, id: &str) -> String {
        fs::read_to_string(self.file(id)).unwrap()
    }

    fn theme(&self, id: &str) -> ThemeFile {
        list(&self.paths)
            .themes
            .into_iter()
            .find(|theme| theme.id == id)
            .unwrap_or_else(|| panic!("no theme {id}"))
    }

    fn create(&self, name: &str) -> String {
        create(&self.paths, name, ThemeKind::Dark, None, &Colors::default()).unwrap()
    }
}

fn colors() -> Colors {
    Colors {
        background: Some("#191513".to_owned()),
        surface: Some("#231D1A".to_owned()),
        danger: Some("#ec625a".to_owned()),
        ..Colors::default()
    }
}

#[test]
fn built_in_ids_and_kinds_are_in_the_plans_order() {
    let table: Vec<(&str, &str)> = BUILT_IN
        .iter()
        .map(|(id, kind)| (*id, kind.as_str()))
        .collect();
    assert_eq!(
        table,
        [
            ("midnight", "dark"),
            ("daylight", "light"),
            ("cellar", "dark"),
            ("cellar-light", "light"),
            ("high-contrast", "dark"),
            ("dracula", "dark"),
            ("catppuccin-macchiato", "dark"),
            ("catppuccin-latte", "light"),
        ]
    );
    assert_eq!(builtin_kind("daylight"), Some(ThemeKind::Light));
    assert_eq!(builtin_kind("custom:daylight"), None);
    assert_eq!(builtin_kind("nope"), None);
}

#[test]
fn the_themes_folder_follows_the_selected_root() {
    let fixture = Fixture::new();
    assert!(fixture.paths.themes_dir().ends_with(".config/wineo/themes"));

    let portable = tempfile::tempdir().unwrap();
    let home = portable.path().join("home");
    let usb = portable.path().join("usb");
    let paths = Paths::select(
        |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&usb),
    )
    .unwrap();
    assert_eq!(paths.themes_dir(), usb.join("config/themes"));
    let id = create(&paths, "Mine", ThemeKind::Light, None, &Colors::default()).unwrap();
    assert!(paths.themes_dir().join(format!("{id}.toml")).is_file());
    assert!(!fixture.paths.themes_dir().exists());
}

#[test]
fn a_missing_folder_is_an_empty_list() {
    let fixture = Fixture::new();
    assert_eq!(list(&fixture.paths), Themes::default());
    assert!(!fixture.paths.themes_dir().exists());
}

#[test]
fn a_created_theme_reads_back_and_matches_the_plans_format() {
    let fixture = Fixture::new();
    let id = create(
        &fixture.paths,
        "My Cellar",
        ThemeKind::Dark,
        Some("cellar"),
        &colors(),
    )
    .unwrap();
    assert_eq!(id, "my-cellar");
    assert_eq!(
        fixture.read(&id),
        "schema_version = 1\nname = \"My Cellar\"\nkind = \"dark\"\nbased_on = \"cellar\"\n\n[colors]\nbackground = \"#191513\"\nsurface = \"#231d1a\"\ndanger = \"#ec625a\"\n"
    );
    assert_eq!(
        fixture.theme(&id),
        ThemeFile {
            id,
            name: "My Cellar".to_owned(),
            kind: Some(ThemeKind::Dark),
            based_on: Some("cellar".to_owned()),
            colors: Colors {
                surface: Some("#231d1a".to_owned()),
                ..colors()
            },
            newer_version: false,
        }
    );
}

#[test]
fn ids_are_slugs_with_a_counter_on_a_clash() {
    let fixture = Fixture::new();
    assert_eq!(fixture.create("Cellar copy"), "cellar-copy");
    assert_eq!(fixture.create("Cellar copy"), "cellar-copy-2");
    assert_eq!(fixture.create("Cellar copy"), "cellar-copy-3");
    assert_eq!(
        fixture.create("  --Dracula!!  Night__v2 "),
        "dracula-night-v2"
    );
    assert_eq!(fixture.create("Caf\u{e9}"), "caf");
    // A name with no letters or digits still gets a file.
    assert_eq!(fixture.create("\u{2603}"), "theme");
    assert_eq!(fixture.create("!!!"), "theme-2");
    // A file that doesn't parse still holds its id.
    fixture.write("held", "name = \n");
    assert_eq!(fixture.create("Held"), "held-2");
}

#[test]
fn a_created_theme_with_a_bad_color_or_based_on() {
    let fixture = Fixture::new();
    let bad = Colors {
        background: Some("red".to_owned()),
        ..Colors::default()
    };
    let error = create(&fixture.paths, "X", ThemeKind::Dark, None, &bad).unwrap_err();
    assert!(
        error.to_string().contains("background: use #rrggbb"),
        "{error}"
    );
    assert!(!fixture.file("x").exists());

    // Only a built-in id is kept.
    let id = create(
        &fixture.paths,
        "Y",
        ThemeKind::Light,
        Some("custom:x"),
        &Colors::default(),
    )
    .unwrap();
    assert!(!fixture.read(&id).contains("based_on"));
}

#[test]
fn lenient_reads_turn_bad_values_into_none() {
    let fixture = Fixture::new();
    fixture.write(
        "hand",
        r##"schema_version = 1
kind = "sepia"
based_on = "custom:other"
future = "ignored"

[colors]
background = "#ABCDEF"
surface = "abcdef"
border = "#abc"
foreground = "#abcdeg"
muted = 5
primary = "#12345678"
success = "#00FF00"
extra = "#000000"
"##,
    );
    let theme = fixture.theme("hand");
    assert_eq!(theme.name, "hand");
    assert_eq!(theme.kind, None);
    assert_eq!(theme.based_on, None);
    assert_eq!(
        theme.colors,
        Colors {
            background: Some("#abcdef".to_owned()),
            success: Some("#00ff00".to_owned()),
            ..Colors::default()
        }
    );
    assert!(!theme.newer_version);

    fixture.write("bare", "");
    let bare = fixture.theme("bare");
    assert_eq!(
        (bare.kind, bare.based_on, bare.colors),
        (None, None, Colors::default())
    );

    fixture.write(
        "unknown",
        "based_on = \"nope\"\nkind = 4\nname = 7\ncolors = \"x\"\n",
    );
    let unknown = fixture.theme("unknown");
    assert_eq!(unknown.based_on, None);
    assert_eq!(unknown.kind, None);
    assert_eq!(unknown.name, "unknown");
    assert_eq!(unknown.colors, Colors::default());
}

#[test]
fn list_sorts_by_name_then_id_and_keeps_unparseable_files_apart() {
    let fixture = Fixture::new();
    fixture.write("z", "name = \"Alpha\"\n");
    fixture.write("b", "name = \"beta\"\n");
    fixture.write("a", "name = \"Alpha\"\n");
    fixture.write("broken", "name = \n");
    fixture.write("old", "schema_version = 0\n");
    fixture.write("typed", "schema_version = \"1\"\n");
    fs::write(fixture.paths.themes_dir().join("notes.txt"), "x").unwrap();
    fs::create_dir(fixture.paths.themes_dir().join("dir.toml")).unwrap();

    let found = list(&fixture.paths);
    let ids: Vec<&str> = found.themes.iter().map(|theme| theme.id.as_str()).collect();
    assert_eq!(ids, ["a", "z", "b"]);
    let invalid: Vec<(&str, &str)> = found
        .invalid
        .iter()
        .map(|theme| (theme.id.as_str(), theme.error.as_str()))
        .collect();
    assert_eq!(invalid.len(), 3, "{invalid:?}");
    assert_eq!(invalid[0].0, "broken");
    assert!(invalid[0].1.starts_with("line 1: "), "{invalid:?}");
    assert_eq!(invalid[1], ("old", "unsupported schema_version 0"));
    assert_eq!(invalid[2], ("typed", "unsupported schema_version \"1\""));
    assert_eq!(found.invalid[0].path, fixture.file("broken"));
}

#[test]
fn a_newer_schema_reads_but_refuses_edits() {
    let fixture = Fixture::new();
    let text = "schema_version = 2\nname = \"Future\"\nkind = \"light\"\n";
    fixture.write("future", text);
    let theme = fixture.theme("future");
    assert!(theme.newer_version);
    assert_eq!(theme.kind, Some(ThemeKind::Light));

    let update_error = update(&fixture.paths, "future", ThemeKind::Dark, None, &colors());
    assert!(matches!(update_error, Err(Error::SchemaVersion { .. })));
    let rename_error = rename(&fixture.paths, "future", "Other");
    assert!(matches!(rename_error, Err(Error::SchemaVersion { .. })));
    assert_eq!(fixture.read("future"), text);

    // Deleting is an explicit ask, not a clobber.
    assert!(delete(&fixture.paths, "future").unwrap());
}

#[test]
fn update_rewrites_colors_kind_and_based_on_and_keeps_the_rest() {
    let fixture = Fixture::new();
    fixture.write(
        "mine",
        "# my theme\nschema_version = 1\nname = \"Mine\"\nkind = \"dark\" # base mode\nbased_on = \"cellar\"\nfuture = \"kept\"\n\n[colors]\n# the page\nbackground = \"#000000\"\nborder = \"#111111\"\nfuture_role = \"kept\"\n",
    );

    let new = Colors {
        background: Some("#FFFFFF".to_owned()),
        surface: Some("#222222".to_owned()),
        ..Colors::default()
    };
    assert!(
        update(
            &fixture.paths,
            "mine",
            ThemeKind::Light,
            Some("daylight"),
            &new
        )
        .unwrap()
    );
    assert_eq!(
        fixture.read("mine"),
        "# my theme\nschema_version = 1\nname = \"Mine\"\nkind = \"light\" # base mode\nbased_on = \"daylight\"\nfuture = \"kept\"\n\n[colors]\n# the page\nbackground = \"#ffffff\"\nfuture_role = \"kept\"\nsurface = \"#222222\"\n"
    );
    let theme = fixture.theme("mine");
    assert_eq!(theme.kind, Some(ThemeKind::Light));
    assert_eq!(theme.based_on.as_deref(), Some("daylight"));
    assert_eq!(
        theme.colors,
        Colors {
            background: Some("#ffffff".to_owned()),
            surface: Some("#222222".to_owned()),
            ..Colors::default()
        }
    );

    // None removes the key.
    assert!(
        update(
            &fixture.paths,
            "mine",
            ThemeKind::Light,
            None,
            &Colors::default()
        )
        .unwrap()
    );
    assert!(!fixture.read("mine").contains("based_on"));
    assert_eq!(fixture.theme("mine").colors, Colors::default());
}

#[test]
fn a_no_op_update_or_rename_does_not_rewrite_the_file() {
    let fixture = Fixture::new();
    fixture.write(
        "mine",
        "schema_version = 1\nname   =   \"Mine\"\nkind = \"dark\"\n\n[colors]\nbackground = \"#ABCDEF\"\n",
    );
    let path = fixture.file("mine");
    let inode = fs::metadata(&path).unwrap().ino();
    let bytes = fs::read(&path).unwrap();
    let same = Colors {
        background: Some("#abcdef".to_owned()),
        ..Colors::default()
    };

    assert!(!update(&fixture.paths, "mine", ThemeKind::Dark, None, &same).unwrap());
    assert!(!rename(&fixture.paths, "mine", "Mine").unwrap());
    assert!(!delete(&fixture.paths, "absent").unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
}

#[test]
fn update_and_rename_of_a_missing_or_broken_file_fail() {
    let fixture = Fixture::new();
    assert!(matches!(
        update(
            &fixture.paths,
            "gone",
            ThemeKind::Dark,
            None,
            &Colors::default()
        ),
        Err(Error::UnknownTheme(_))
    ));
    assert!(matches!(
        rename(&fixture.paths, "gone", "X"),
        Err(Error::UnknownTheme(_))
    ));
    fixture.write("broken", "name = \n");
    assert!(matches!(
        rename(&fixture.paths, "broken", "X"),
        Err(Error::ConfigParse { .. })
    ));
    assert_eq!(fixture.read("broken"), "name = \n");

    for id in ["", "..", "a/b"] {
        assert!(matches!(
            delete(&fixture.paths, id),
            Err(Error::InvalidThemeId(_))
        ));
    }
}

#[test]
fn rename_changes_only_the_name_and_keeps_the_id_and_file() {
    let fixture = Fixture::new();
    let id = create(
        &fixture.paths,
        "Cellar copy",
        ThemeKind::Dark,
        Some("cellar"),
        &colors(),
    )
    .unwrap();
    let before = fixture.read(&id);

    assert!(rename(&fixture.paths, &id, "Mine").unwrap());
    assert_eq!(fixture.read(&id), before.replace("Cellar copy", "Mine"));
    assert_eq!(fixture.theme(&id).name, "Mine");
    assert!(fixture.file("mine").metadata().is_err());
    // A second copy of the same source still gets the next id.
    assert_eq!(fixture.create("Cellar copy"), "cellar-copy-2");
}

#[test]
fn rename_to_a_taken_name_says_name_taken_ignoring_case() {
    let fixture = Fixture::new();
    let first = fixture.create("First");
    let second = fixture.create("Second");
    // A file added by hand with no name is known by its id.
    fixture.write("Hand", "kind = \"dark\"\n");

    for taken in ["First", "first", "FIRST", " first ", "hand"] {
        let error = rename(&fixture.paths, &second, taken).unwrap_err();
        assert!(matches!(error, Error::ThemeNameTaken), "{taken}");
        assert_eq!(error.to_string(), "Name taken.");
    }
    assert_eq!(fixture.theme(&second).name, "Second");

    // Its own name in another case is allowed.
    assert!(rename(&fixture.paths, &first, "FIRST").unwrap());
    assert_eq!(fixture.theme(&first).name, "FIRST");
}

#[test]
fn delete_removes_the_file_and_a_missing_one_is_not_an_error() {
    let fixture = Fixture::new();
    let id = fixture.create("Gone");
    assert!(delete(&fixture.paths, &id).unwrap());
    assert!(!fixture.file(&id).exists());
    assert!(!delete(&fixture.paths, &id).unwrap());
    assert_eq!(list(&fixture.paths), Themes::default());
}

#[test]
fn normalizing_colors() {
    assert_eq!(normalize_color("#ABCdef").as_deref(), Some("#abcdef"));
    for bad in [
        "", "#", "abcdef", "#abcde", "#abcdeff", "#ggg000", " #abcdef", "#abcdef ",
    ] {
        assert_eq!(normalize_color(bad), None, "{bad}");
    }
}
