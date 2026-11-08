use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;
use crate::library::{self, Library, load, preset_name_taken};

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

    fn with(text: &str) -> Self {
        let fixture = Self::new();
        fixture.write(text);
        fixture
    }

    fn write(&self, text: &str) {
        fs::create_dir_all(self.paths.library_file().parent().unwrap()).unwrap();
        fs::write(self.paths.library_file(), text).unwrap();
    }

    fn read(&self) -> String {
        fs::read_to_string(self.paths.library_file()).unwrap()
    }

    fn library(&self) -> Library {
        load(&self.paths)
    }

    fn presets(&self) -> Vec<String> {
        self.library()
            .presets
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    fn preset(&self, name: &str) -> Preset {
        self.library().preset(name).cloned().expect(name)
    }

    fn inode(&self) -> u64 {
        fs::metadata(self.paths.library_file()).unwrap().ino()
    }
}

fn set<T: Ord>(items: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    items.into_iter().collect()
}

fn tags(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|tag| (*tag).to_owned()).collect()
}

/// The preset from decision 14's example.
fn couch() -> Preset {
    Preset {
        sort: SortKey::LastPlayed,
        descending: true,
        favorites_first: false,
        stores: set([Store::Steam]),
        tags: tags(&["co-op"]),
        not_tags: tags(&["vr"]),
        group: GroupBy::Tag,
        layout: ViewLayout::List,
        ..Preset::default()
    }
}

/// Every field set to something other than its default.
fn everything() -> Preset {
    Preset {
        sort: SortKey::Tags,
        descending: true,
        favorites_first: true,
        stores: set([Store::Gog, Store::Steam]),
        platforms: set([Platform::Native]),
        tags: tags(&["co-op", "rpg"]),
        not_stores: set([Store::Epic, Store::Amazon]),
        not_platforms: set([Platform::Windows]),
        not_tags: tags(&["vr", "demo"]),
        show_hidden: true,
        group: GroupBy::State,
        layout: ViewLayout::List,
    }
}

#[test]
fn a_new_preset_is_written_in_the_shape_decision_14_gives() {
    let fixture = Fixture::new();
    assert!(set_preset(&fixture.paths, "Couch co-op", &couch()).unwrap());
    assert_eq!(
        fixture.read(),
        r#"schema_version = 1

[presets."Couch co-op"]
sort = "last-played"
descending = true
favorites_first = false
stores = ["steam"]
tags = ["co-op"]
not_tags = ["vr"]
show_hidden = false
group = "tag"
layout = "list"
"#
    );
    assert_eq!(fixture.preset("Couch co-op"), couch());
}

#[test]
fn a_preset_holds_its_filters_and_layout_and_nothing_of_search_or_zoom() {
    let fixture = Fixture::new();
    set_preset(&fixture.paths, "Everything", &everything()).unwrap();
    let text = fixture.read();
    assert_eq!(
        text,
        r#"schema_version = 1

[presets.Everything]
sort = "tags"
descending = true
favorites_first = true
stores = ["gog", "steam"]
platforms = ["native"]
tags = ["co-op", "rpg"]
not_stores = ["amazon", "epic"]
not_platforms = ["windows"]
not_tags = ["demo", "vr"]
show_hidden = true
group = "state"
layout = "list"
"#
    );
    for key in ["search", "zoom", "poster_only", "random"] {
        assert!(!text.contains(key), "{key}");
    }
    assert_eq!(fixture.preset("Everything"), everything());
}

#[test]
fn a_preset_made_of_defaults_names_every_setting_but_no_empty_list() {
    let fixture = Fixture::new();
    assert!(set_preset(&fixture.paths, "Plain", &Preset::default()).unwrap());
    assert_eq!(
        fixture.read(),
        r#"schema_version = 1

[presets.Plain]
sort = "name"
descending = false
favorites_first = false
show_hidden = false
group = "none"
layout = "grid"
"#
    );
    assert_eq!(fixture.preset("Plain"), Preset::default());
}

#[test]
fn every_field_round_trips_alone() {
    let base = Preset::default();
    let cases = [
        Preset {
            sort: SortKey::Source,
            ..base.clone()
        },
        Preset {
            descending: true,
            ..base.clone()
        },
        Preset {
            favorites_first: true,
            ..base.clone()
        },
        Preset {
            stores: set(Store::ALL),
            ..base.clone()
        },
        Preset {
            platforms: set(Platform::ALL),
            ..base.clone()
        },
        Preset {
            tags: tags(&["a", "B", "with space", "dotted.tag"]),
            ..base.clone()
        },
        Preset {
            not_stores: set(Store::ALL),
            ..base.clone()
        },
        Preset {
            not_platforms: set(Platform::ALL),
            ..base.clone()
        },
        Preset {
            not_tags: tags(&["a", "quo\"te", "back\\slash"]),
            ..base.clone()
        },
        Preset {
            show_hidden: true,
            ..base.clone()
        },
        Preset {
            group: GroupBy::Source,
            ..base.clone()
        },
        Preset {
            layout: ViewLayout::List,
            ..base
        },
    ];
    for (index, preset) in cases.iter().enumerate() {
        let fixture = Fixture::new();
        set_preset(&fixture.paths, "P", preset).unwrap();
        assert_eq!(&fixture.preset("P"), preset, "case {index}");
    }
    for sort in SortKey::ALL {
        let fixture = Fixture::new();
        let preset = Preset {
            sort,
            ..Preset::default()
        };
        set_preset(&fixture.paths, "P", &preset).unwrap();
        assert_eq!(fixture.preset("P"), preset, "{sort:?}");
    }
    for group in [
        GroupBy::None,
        GroupBy::Recent,
        GroupBy::Tag,
        GroupBy::Source,
        GroupBy::State,
    ] {
        let fixture = Fixture::new();
        let preset = Preset {
            group,
            ..Preset::default()
        };
        set_preset(&fixture.paths, "P", &preset).unwrap();
        assert_eq!(fixture.preset("P"), preset, "{group:?}");
    }
}

#[test]
fn names_with_dots_quotes_and_unicode_round_trip() {
    let fixture = Fixture::new();
    let names = [
        "a.b",
        "say \"hi\"",
        "back\\slash",
        "[x]",
        "Ünï côdé",
        "42",
        "a b",
    ];
    for name in names {
        set_preset(&fixture.paths, name, &couch()).unwrap();
    }
    let mut expected: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    expected.sort_by_key(|name| (name.to_lowercase(), name.clone()));
    assert_eq!(fixture.presets(), expected);
    for name in names {
        assert_eq!(fixture.preset(name), couch(), "{name}");
    }
}

#[test]
fn presets_list_a_to_z_ignoring_case() {
    let fixture = Fixture::with(
        "[presets.zeta]\n[presets.Beta]\n[presets.alpha]\n[presets.Alpha2]\n[presets.\"beta \"]\n",
    );
    assert_eq!(
        fixture.presets(),
        ["alpha", "Alpha2", "Beta", "beta ", "zeta"]
    );
}

#[test]
fn a_file_with_no_presets_table_reads_as_none() {
    let fixture = Fixture::with("schema_version = 1\n[view]\nsort = \"added\"\n");
    assert_eq!(fixture.library().presets, []);
    assert_eq!(Library::default().presets, []);
}

#[test]
fn an_unknown_field_is_ignored_and_kept_when_the_preset_is_updated() {
    let original = r#"schema_version = 1

# my favorite view
[presets."Couch co-op"]
# how it sorts
sort = "last-played"
descending = true
favorites_first = false
stores = ["steam"]
tags = ["co-op"]
not_tags = ["vr"]
show_hidden = false
group = "tag"
layout = "list"
future_field = { nested = 1 }  # from a newer wineo
notes = "keep me"
"#;
    let fixture = Fixture::with(original);
    assert_eq!(fixture.preset("Couch co-op"), couch());

    let changed = Preset {
        sort: SortKey::Playtime,
        ..couch()
    };
    assert!(set_preset(&fixture.paths, "Couch co-op", &changed).unwrap());
    assert_eq!(
        fixture.read(),
        original.replace("sort = \"last-played\"", "sort = \"playtime\"")
    );
    assert_eq!(fixture.preset("Couch co-op"), changed);
}

#[test]
fn a_bad_value_in_one_field_falls_back_for_that_field_alone() {
    let fixture = Fixture::with(
        r#"schema_version = 1
[presets.Mixed]
sort = "random"
descending = "yes"
favorites_first = true
stores = "steam"
platforms = ["windows"]
tags = ["co-op", 3, true]
not_stores = [1]
show_hidden = 1
group = "tag"
layout = ["list"]
"#,
    );
    assert_eq!(
        fixture.preset("Mixed"),
        Preset {
            favorites_first: true,
            platforms: set([Platform::Windows]),
            tags: tags(&["co-op"]),
            group: GroupBy::Tag,
            ..Preset::default()
        }
    );
}

#[test]
fn an_unknown_filter_id_costs_only_itself() {
    let fixture = Fixture::with(
        r#"[presets.Future]
sort = "added"
stores = ["steam", "itch", "GOG", "gog"]
platforms = ["macos"]
not_platforms = ["windows", "macos"]
tags = ["co-op"]
layout = "list"
"#,
    );
    assert_eq!(
        fixture.preset("Future"),
        Preset {
            sort: SortKey::Added,
            stores: set([Store::Steam, Store::Gog]),
            not_platforms: set([Platform::Windows]),
            tags: tags(&["co-op"]),
            layout: ViewLayout::List,
            ..Preset::default()
        }
    );
}

#[test]
fn entries_that_are_not_presets_are_skipped_and_the_rest_read() {
    let fixture = Fixture::with(
        "[presets]\nbroken = 1\ntext = \"x\"\n[presets.Good]\nsort = \"added\"\n[presets.\" \"]\nsort = \"added\"\n[presets.\"\"]\n",
    );
    assert_eq!(fixture.presets(), ["Good"]);

    let fixture = Fixture::with("presets = 1\n[view]\nsort = \"added\"\n");
    assert_eq!(fixture.library().presets, []);
    assert_eq!(fixture.library().view.sort, SortKey::Added);

    // An inline table is a table too.
    let fixture = Fixture::with("[presets]\nInline = { sort = \"playtime\", descending = true }\n");
    assert_eq!(
        fixture.preset("Inline"),
        Preset {
            sort: SortKey::Playtime,
            descending: true,
            ..Preset::default()
        }
    );
}

#[test]
fn a_tag_nobody_has_any_more_is_kept() {
    let fixture = Fixture::new();
    let preset = Preset {
        tags: tags(&["deleted-tag"]),
        ..Preset::default()
    };
    set_preset(&fixture.paths, "Old", &preset).unwrap();
    assert_eq!(fixture.preset("Old").tags, tags(&["deleted-tag"]));
}

#[test]
fn presets_do_not_disturb_the_view_or_entries_they_sit_beside() {
    let fixture = Fixture::with(
        "schema_version = 1\n\n# the view\n[view]\nsort = \"added\" # newest first\ndescending = true\n\n[environments.hades]\nfavorite = true\n",
    );
    let before = fixture.library();
    set_preset(&fixture.paths, "Couch", &couch()).unwrap();
    let after = fixture.library();
    assert_eq!(after.view, before.view);
    assert_eq!(after.environments, before.environments);
    let text = fixture.read();
    assert!(text.starts_with(
        "schema_version = 1\n\n# the view\n[view]\nsort = \"added\" # newest first\ndescending = true\n\n[environments.hades]\nfavorite = true\n"
    ), "{text}");
    assert!(text.contains("[presets.Couch]"), "{text}");
}

#[test]
fn updating_writes_only_the_lines_that_changed() {
    let fixture = Fixture::new();
    set_preset(&fixture.paths, "P", &couch()).unwrap();
    let before = fixture.read();

    let changed = Preset {
        descending: false,
        stores: set([Store::Steam, Store::Gog]),
        not_tags: tags(&[]),
        ..couch()
    };
    assert!(set_preset(&fixture.paths, "P", &changed).unwrap());
    let expected = before
        .replace("descending = true", "descending = false")
        .replace("stores = [\"steam\"]", "stores = [\"steam\", \"gog\"]")
        .replace("not_tags = [\"vr\"]\n", "");
    assert_eq!(fixture.read(), expected);
    assert_eq!(fixture.preset("P"), changed);
}

#[test]
fn a_comment_above_a_key_and_after_its_value_stay_through_an_update() {
    let fixture = Fixture::with(
        "[presets.P]\n# why this sort\nsort = \"added\" # newest first\ndescending = false\ntags = [\"a\"] # for sure\n",
    );
    let preset = Preset {
        sort: SortKey::Playtime,
        tags: tags(&["a", "b"]),
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &preset).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\n# why this sort\nsort = \"playtime\" # newest first\ndescending = false\ntags = [\"a\", \"b\"] # for sure\n"
    );
}

#[test]
fn saving_what_the_file_already_reads_as_writes_nothing() {
    let fixture = Fixture::new();
    set_preset(&fixture.paths, "P", &couch()).unwrap();
    let bytes = fixture.read();
    let inode = fixture.inode();
    assert!(!set_preset(&fixture.paths, "P", &couch()).unwrap());
    assert_eq!(fixture.read(), bytes);
    assert_eq!(fixture.inode(), inode);

    // A hand-written preset that leaves defaults out reads as the same
    // preset, so saving it unchanged isn't an edit either.
    let written = "# hand made\n[presets.Mine]\nsort = \"playtime\"\ntags = [ \"a\",\"b\" ]\n";
    let fixture = Fixture::with(written);
    let inode = fixture.inode();
    let preset = Preset {
        sort: SortKey::Playtime,
        tags: tags(&["a", "b"]),
        ..Preset::default()
    };
    assert_eq!(fixture.preset("Mine"), preset);
    assert!(!set_preset(&fixture.paths, "Mine", &preset).unwrap());
    assert_eq!(fixture.read(), written);
    assert_eq!(fixture.inode(), inode);

    // An empty table with nothing but defaults to save is the same.
    let fixture = Fixture::with("[presets.Empty]\n");
    assert!(!set_preset(&fixture.paths, "Empty", &Preset::default()).unwrap());
    assert_eq!(fixture.read(), "[presets.Empty]\n");
}

#[test]
fn reads_and_failed_saves_never_create_the_file() {
    let fixture = Fixture::new();
    let _ = fixture.library();
    assert!(!remove_preset(&fixture.paths, "P").unwrap());
    assert!(matches!(
        rename_preset(&fixture.paths, "P", "Q"),
        Err(Error::UnknownPreset(_))
    ));
    assert!(matches!(
        set_preset(&fixture.paths, " ", &couch()),
        Err(Error::InvalidPresetName(_))
    ));
    assert!(!fixture.paths.library_file().exists());
}

#[test]
fn ids_a_newer_wineo_wrote_survive_an_update_that_leaves_that_list_alone() {
    let fixture = Fixture::with(
        "[presets.P]\nsort = \"added\"\nstores = [\"steam\", \"itch\"]\nplatforms = [\"macos\"]\n",
    );
    let known = fixture.preset("P");
    assert_eq!(known.stores, set([Store::Steam]));

    let changed = Preset {
        sort: SortKey::Name,
        ..known.clone()
    };
    assert!(set_preset(&fixture.paths, "P", &changed).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\nsort = \"name\"\nstores = [\"steam\", \"itch\"]\nplatforms = [\"macos\"]\n"
    );

    // Changing the list itself replaces it.
    let changed = Preset {
        stores: set([Store::Steam, Store::Epic]),
        ..changed
    };
    assert!(set_preset(&fixture.paths, "P", &changed).unwrap());
    assert!(
        fixture
            .read()
            .contains("stores = [\"steam\", \"itch\", \"epic\"]\nplatforms = [\"macos\"]")
    );
}

#[test]
fn an_empty_filter_removes_its_key_and_a_bad_one_is_replaced() {
    let fixture = Fixture::with(
        "[presets.P]\nstores = [\"steam\"]\ntags = \"co-op\"\nnot_tags = [3]\nnot_stores = []\n",
    );
    assert!(set_preset(&fixture.paths, "P", &Preset::default()).unwrap());
    // A list that is there but reads empty is already right.
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\nnot_tags = [3]\nnot_stores = []\n"
    );

    let fixture = Fixture::with("[presets.P]\ntags = \"co-op\"\n");
    let preset = Preset {
        tags: tags(&["co-op"]),
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &preset).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\ntags = [\"co-op\"]\n"
    );

    // A bad value where the wanted list is empty is dropped.
    let fixture = Fixture::with("[presets.P]\ntags = \"co-op\"\n");
    assert!(set_preset(&fixture.paths, "P", &Preset::default()).unwrap());
    assert_eq!(fixture.read(), "schema_version = 1\n[presets.P]\n");
}

#[test]
fn a_bad_scalar_is_replaced_even_with_the_default() {
    let fixture = Fixture::with("[presets.P]\nsort = \"random\"\ndescending = \"yes\"\n");
    assert!(set_preset(&fixture.paths, "P", &Preset::default()).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\nsort = \"name\"\ndescending = false\n"
    );
}

#[test]
fn a_preset_does_not_replace_an_entry_that_was_not_a_table() {
    let fixture = Fixture::with("[presets]\nP = 1\n");
    assert_eq!(fixture.presets(), Vec::<String>::new());
    assert!(matches!(
        set_preset(&fixture.paths, "P", &couch()),
        Err(Error::PresetNameTaken(taken)) if taken == "P"
    ));
    assert_eq!(fixture.read(), "[presets]\nP = 1\n");
}

#[test]
fn bad_names_are_refused_before_anything_is_written() {
    let long = "x".repeat(PRESET_NAME_MAX_CHARS + 1);
    let fine = "x".repeat(PRESET_NAME_MAX_CHARS);
    let cases = [
        ("", "Invalid preset name: can't be empty."),
        ("   ", "Invalid preset name: can't be empty."),
        ("\t", "Invalid preset name: can't be empty."),
        (
            " lead",
            "Invalid preset name: can't start or end with whitespace.",
        ),
        (
            "trail ",
            "Invalid preset name: can't start or end with whitespace.",
        ),
        (
            "a\nb",
            "Invalid preset name: can't contain control characters.",
        ),
        (
            long.as_str(),
            "Invalid preset name: can't be longer than 60 characters.",
        ),
    ];
    for (name, message) in cases {
        let fixture = Fixture::new();
        let error = set_preset(&fixture.paths, name, &couch()).unwrap_err();
        assert!(matches!(error, Error::InvalidPresetName(_)), "{name:?}");
        assert_eq!(error.to_string(), message, "{name:?}");
        assert!(!fixture.paths.library_file().exists(), "{name:?}");
        assert_eq!(preset_name_problem(name).as_deref(), Some(message));
    }
    let fixture = Fixture::new();
    assert!(set_preset(&fixture.paths, &fine, &couch()).unwrap());
    assert_eq!(preset_name_problem("Couch co-op"), None);
    assert_eq!(preset_name_problem("it's \"fine\""), None);
}

#[test]
fn a_typed_name_is_trimmed_then_checked() {
    assert_eq!(
        normalize_preset_name("  Couch co-op \n").unwrap(),
        "Couch co-op"
    );
    assert_eq!(
        normalize_preset_name("   ").unwrap_err(),
        "Invalid preset name: can't be empty."
    );
    assert_eq!(
        normalize_preset_name("").unwrap_err(),
        "Invalid preset name: can't be empty."
    );
    assert!(normalize_preset_name(&"y".repeat(61)).is_err());
}

#[test]
fn a_name_that_differs_only_in_case_is_taken_but_the_exact_name_updates() {
    let fixture = Fixture::new();
    set_preset(&fixture.paths, "Couch co-op", &couch()).unwrap();
    let bytes = fixture.read();

    for name in ["couch CO-OP", "COUCH CO-OP"] {
        let error = set_preset(&fixture.paths, name, &Preset::default()).unwrap_err();
        assert!(matches!(&error, Error::PresetNameTaken(taken) if taken == "Couch co-op"));
        assert_eq!(error.to_string(), "The name Couch co-op is taken.");
        assert_eq!(fixture.read(), bytes);
    }

    let library = fixture.library();
    assert_eq!(
        library.taken_preset_name("couch co-op"),
        Some("Couch co-op")
    );
    assert_eq!(
        library.taken_preset_name("Couch co-op"),
        Some("Couch co-op")
    );
    assert_eq!(library.taken_preset_name("Couch"), None);

    let changed = Preset {
        descending: false,
        ..couch()
    };
    assert!(set_preset(&fixture.paths, "Couch co-op", &changed).unwrap());
    assert_eq!(fixture.presets(), ["Couch co-op"]);
}

#[test]
fn the_exact_name_wins_when_a_file_holds_two_that_differ_in_case() {
    let fixture =
        Fixture::with("[presets.A]\nsort = \"added\"\n[presets.a]\nsort = \"playtime\"\n");
    let library = fixture.library();
    assert_eq!(library.taken_preset_name("a"), Some("a"));
    assert_eq!(library.taken_preset_name("A"), Some("A"));
    assert_eq!(library.taken_preset_name("b"), None);
    assert!(set_preset(&fixture.paths, "a", &Preset::default()).unwrap());
    assert_eq!(fixture.preset("A").sort, SortKey::Added);
    assert_eq!(fixture.preset("a").sort, SortKey::Name);
}

#[test]
fn removing_drops_only_that_preset_and_leaves_the_rest_byte_for_byte() {
    let text = "schema_version = 1\n\n# keep\n[view]\nsort = \"added\"\n\n[presets.Keep]\n# note\nsort = \"playtime\"\n\n[presets.Drop]\nsort = \"added\"\nfuture = 1\n\n[presets.Also]\nlayout = \"list\" # c\n\n[environments.hades]\nfavorite = true\n";
    let fixture = Fixture::with(text);
    assert!(remove_preset(&fixture.paths, "Drop").unwrap());
    assert_eq!(
        fixture.read(),
        text.replace("[presets.Drop]\nsort = \"added\"\nfuture = 1\n\n", "")
    );
    assert_eq!(fixture.presets(), ["Also", "Keep"]);

    let bytes = fixture.read();
    let inode = fixture.inode();
    // Gone already, a different case, and an entry that isn't a preset.
    assert!(!remove_preset(&fixture.paths, "Drop").unwrap());
    assert!(!remove_preset(&fixture.paths, "keep").unwrap());
    assert!(!remove_preset(&fixture.paths, "").unwrap());
    assert_eq!(fixture.read(), bytes);
    assert_eq!(fixture.inode(), inode);
}

#[test]
fn removing_the_last_preset_leaves_a_file_that_still_reads() {
    let fixture = Fixture::new();
    set_preset(&fixture.paths, "Only", &couch()).unwrap();
    assert!(remove_preset(&fixture.paths, "Only").unwrap());
    assert_eq!(fixture.read(), "schema_version = 1\n");
    assert_eq!(fixture.presets(), Vec::<String>::new());
    assert!(set_preset(&fixture.paths, "Again", &couch()).unwrap());
    assert_eq!(fixture.presets(), ["Again"]);
}

#[test]
fn removing_leaves_a_broken_entry_alone() {
    let text = "[presets]\nbroken = 1\n";
    let fixture = Fixture::with(text);
    assert!(!remove_preset(&fixture.paths, "broken").unwrap());
    assert_eq!(fixture.read(), text);
}

#[test]
fn renaming_moves_the_table_whole_in_place() {
    let text = "schema_version = 1\n\n[presets.Alpha]\nsort = \"added\"\n\n# this one is mine\n[presets.\"Old name\"]\n# why\nsort = \"playtime\" # trailing\nstores = [\"gog\"]\nfuture = { a = 1 }\n\n[presets.Zed]\nsort = \"tags\"\n\n[environments.hades]\nfavorite = true\n";
    let fixture = Fixture::with(text);
    let preset = fixture.preset("Old name");

    assert!(rename_preset(&fixture.paths, "Old name", "New name").unwrap());
    assert_eq!(
        fixture.read(),
        text.replace("[presets.\"Old name\"]", "[presets.\"New name\"]")
    );
    assert_eq!(fixture.presets(), ["Alpha", "New name", "Zed"]);
    assert_eq!(fixture.preset("New name"), preset);
    assert!(fixture.library().preset("Old name").is_none());
}

#[test]
fn renaming_to_a_taken_name_is_refused_whatever_its_case() {
    let text = "[presets.Couch]\nsort = \"added\"\n[presets.Desk]\nsort = \"playtime\"\n";
    let fixture = Fixture::with(text);
    for new in ["Desk", "desk", "DESK"] {
        let error = rename_preset(&fixture.paths, "Couch", new).unwrap_err();
        assert!(
            matches!(&error, Error::PresetNameTaken(taken) if taken == "Desk"),
            "{new}"
        );
        assert_eq!(error.to_string(), "The name Desk is taken.");
        assert_eq!(fixture.read(), text, "{new}");
    }
}

#[test]
fn renaming_to_the_same_name_writes_nothing() {
    let text = "[presets.Couch]\nsort = \"added\"\n";
    let fixture = Fixture::with(text);
    let inode = fixture.inode();
    assert!(!rename_preset(&fixture.paths, "Couch", "Couch").unwrap());
    assert_eq!(fixture.read(), text);
    assert_eq!(fixture.inode(), inode);

    // Even a name nobody could type now, as long as it is the same one.
    let fixture = Fixture::with("[presets.\" odd\"]\n");
    assert!(!rename_preset(&fixture.paths, " odd", " odd").unwrap());
}

#[test]
fn renaming_changes_only_the_case_of_its_own_name() {
    let fixture = Fixture::with("[presets.couch]\nsort = \"added\"\n[presets.Desk]\n");
    assert!(rename_preset(&fixture.paths, "couch", "Couch").unwrap());
    assert_eq!(fixture.presets(), ["Couch", "Desk"]);
    assert_eq!(fixture.preset("Couch").sort, SortKey::Added);
    // It still can't take another preset's name in another case.
    assert!(matches!(
        rename_preset(&fixture.paths, "Couch", "desk"),
        Err(Error::PresetNameTaken(_))
    ));
}

#[test]
fn renaming_refuses_blank_and_bad_new_names() {
    let text = "[presets.Couch]\nsort = \"added\"\n";
    let fixture = Fixture::with(text);
    for new in ["", "   ", " Couch", "Couch ", "a\tb"] {
        let error = rename_preset(&fixture.paths, "Couch", new).unwrap_err();
        assert!(matches!(error, Error::InvalidPresetName(_)), "{new:?}");
        assert_eq!(fixture.read(), text, "{new:?}");
    }
}

#[test]
fn renaming_a_preset_that_is_gone_says_so() {
    let fixture = Fixture::with("[presets.Couch]\n");
    let error = rename_preset(&fixture.paths, "Missing", "Other").unwrap_err();
    assert!(matches!(&error, Error::UnknownPreset(name) if name == "Missing"));
    assert_eq!(error.to_string(), "There is no preset named Missing.");
    // The wrong case is another name.
    assert!(matches!(
        rename_preset(&fixture.paths, "couch", "Other"),
        Err(Error::UnknownPreset(_))
    ));
    let fixture = Fixture::with("[presets]\nbroken = 1\n");
    assert!(matches!(
        rename_preset(&fixture.paths, "broken", "Other"),
        Err(Error::UnknownPreset(_))
    ));
}

#[test]
fn a_rename_is_one_write_so_an_unreadable_file_keeps_the_old_name() {
    for text in [
        "[presets.Couch]\nsort = \"added\"\nthis is broken\n",
        "schema_version = 2\n[presets.Couch]\n",
        "schema_version = \"1\"\n[presets.Couch]\n",
    ] {
        let fixture = Fixture::with(text);
        let refused = |result: Result<bool>| {
            assert!(
                matches!(result, Err(Error::LibraryUnreadable { .. })),
                "{text}: {result:?}"
            );
        };
        refused(set_preset(&fixture.paths, "Couch", &couch()));
        refused(set_preset(&fixture.paths, "New", &couch()));
        refused(remove_preset(&fixture.paths, "Couch"));
        refused(rename_preset(&fixture.paths, "Couch", "Renamed"));
        assert_eq!(fixture.read(), text);
    }
}

#[test]
fn a_file_that_is_not_text_is_refused_and_left_alone() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.paths.library_file().parent().unwrap()).unwrap();
    fs::write(fixture.paths.library_file(), b"[presets.A]\n\xff\xfe\n").unwrap();
    assert!(set_preset(&fixture.paths, "A", &couch()).is_err());
    assert!(remove_preset(&fixture.paths, "A").is_err());
    assert!(rename_preset(&fixture.paths, "A", "B").is_err());
    assert_eq!(
        fs::read(fixture.paths.library_file()).unwrap(),
        b"[presets.A]\n\xff\xfe\n"
    );
}

#[test]
fn a_file_a_newer_wineo_wrote_says_so_when_a_save_is_refused() {
    let fixture = Fixture::with("schema_version = 2\n[presets.A]\n");
    let error = set_preset(&fixture.paths, "A", &couch()).unwrap_err();
    assert!(matches!(
        error,
        Error::LibraryUnreadable {
            newer_version: true,
            ..
        }
    ));
}

#[test]
fn an_unreadable_file_reads_as_no_presets_with_the_problem_set() {
    let fixture = Fixture::with("[presets.A]\nbroken\n");
    let library = fixture.library();
    assert_eq!(library.presets, []);
    assert!(library.problem.is_some());
}

#[test]
fn writers_to_different_presets_all_land() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(5));
    let names = ["one", "two", "three", "four"];
    let writers = names.map(|name| {
        let paths = fixture.paths.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            set_preset(&paths, name, &couch()).unwrap()
        })
    });
    barrier.wait();
    assert!(writers.into_iter().all(|writer| writer.join().unwrap()));
    assert_eq!(fixture.presets(), ["four", "one", "three", "two"]);
}

#[test]
fn a_rename_and_a_favorite_at_once_both_land() {
    let fixture = Fixture::with("[presets.Old]\nsort = \"added\"\n");
    let barrier = Arc::new(Barrier::new(3));
    let renamer = {
        let paths = fixture.paths.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            rename_preset(&paths, "Old", "New").unwrap()
        })
    };
    let favoriter = {
        let paths = fixture.paths.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            library::set_favorite(&paths, "hades", true).unwrap()
        })
    };
    barrier.wait();
    assert!(renamer.join().unwrap());
    assert!(favoriter.join().unwrap());
    let library = fixture.library();
    assert_eq!(fixture.presets(), ["New"]);
    assert!(library.environments["hades"].favorite);
}

#[test]
fn two_renames_to_the_same_name_let_one_win_and_refuse_the_other() {
    let fixture = Fixture::with("[presets.A]\n[presets.B]\n");
    let barrier = Arc::new(Barrier::new(3));
    let results = ["A", "B"].map(|old| {
        let paths = fixture.paths.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            rename_preset(&paths, old, "C")
        })
    });
    barrier.wait();
    let outcomes: Vec<_> = results
        .into_iter()
        .map(|result| result.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        outcomes
            .iter()
            .any(|result| matches!(result, Err(Error::PresetNameTaken(name)) if name == "C"))
    );
    assert_eq!(fixture.presets().len(), 2);
    assert!(fixture.presets().contains(&"C".to_owned()));
}

#[test]
fn other_writes_keep_the_presets_an_older_wineo_does_not_know() {
    // The code paths an older wineo runs for its own settings are these
    // same writers, which edit the one field and leave unknown tables be.
    let text = "schema_version = 1\n# presets below\n[presets.\"Couch co-op\"]\nsort = \"added\" # c\ngroup = \"tag\"\nstores = [\"steam\"]\n";
    let fixture = Fixture::with(text);
    assert!(library::set_favorite(&fixture.paths, "hades", true).unwrap());
    assert!(library::set_sort(&fixture.paths, SortKey::Playtime).unwrap());
    assert!(library::set_zoom(&fixture.paths, 150).unwrap());
    let after = fixture.read();
    assert!(after.contains(
        "# presets below\n[presets.\"Couch co-op\"]\nsort = \"added\" # c\ngroup = \"tag\"\nstores = [\"steam\"]\n"
    ), "{after}");
    assert_eq!(fixture.preset("Couch co-op").group, GroupBy::Tag);
}

#[test]
fn saving_writes_through_a_symlinked_library_file() {
    use crate::files::link_check::{Dangling, NewFile, Writer};
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let seed = "schema_version = 1\n[presets.P]\nsort = \"added\"\n";
    let preset = Preset {
        sort: SortKey::Added,
        ..Preset::default()
    };
    Writer {
        link: &paths.library_file(),
        seed,
        write: &mut || set_preset(&paths, "Q", &couch()).map(|_| ()),
        no_op: Some((seed, &mut || set_preset(&paths, "P", &preset).map(|_| ()))),
        dangling: Dangling::Refused,
        new_file: NewFile::Default,
    }
    .check();
}

const MULTILINE: &str = r#"[presets.P]
tags = [
  "co-op", # couch only
  # a note on its own line
  "solo", # alone
  "vr",
  # last words
]
not_stores = [ "epic" ] # trailing
"#;

fn multiline_preset() -> Preset {
    Preset {
        tags: tags(&["co-op", "solo", "vr"]),
        not_stores: set([Store::Epic]),
        ..Preset::default()
    }
}

#[test]
fn adding_to_a_list_keeps_every_comment_inside_it() {
    let fixture = Fixture::with(MULTILINE);
    assert_eq!(fixture.preset("P"), multiline_preset());
    let wanted = Preset {
        tags: tags(&["co-op", "solo", "vr", "rpg"]),
        not_stores: set([Store::Epic, Store::Gog]),
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &wanted).unwrap());
    assert_eq!(
        fixture.read(),
        r#"schema_version = 1
[presets.P]
tags = [
  "co-op", # couch only
  # a note on its own line
  "solo", # alone
  "vr",
  "rpg",
  # last words
]
not_stores = [ "epic", "gog" ] # trailing
"#
    );
    assert_eq!(fixture.preset("P"), wanted);
}

#[test]
fn removing_from_a_list_drops_only_that_entrys_own_comment() {
    let fixture = Fixture::with(MULTILINE);
    let wanted = Preset {
        tags: tags(&["co-op", "vr"]),
        not_stores: set([Store::Epic]),
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &wanted).unwrap());
    assert_eq!(
        fixture.read(),
        r#"schema_version = 1
[presets.P]
tags = [
  "co-op", # couch only
  # a note on its own line
  "vr",
  # last words
]
not_stores = [ "epic" ] # trailing
"#
    );
}

#[test]
fn a_list_that_loses_everything_keeps_its_comments_but_a_bare_one_goes() {
    let fixture = Fixture::with(MULTILINE);
    assert!(set_preset(&fixture.paths, "P", &Preset::default()).unwrap());
    let text = fixture.read();
    assert!(text.contains("# a note on its own line"), "{text}");
    assert!(text.contains("# last words"), "{text}");
    assert!(text.contains("not_stores = [ ] # trailing"), "{text}");
    assert_eq!(fixture.preset("P"), Preset::default());

    let fixture = Fixture::with("[presets.P]\ntags = [\"a\", \"b\"]\nsort = \"added\"\n");
    let wanted = Preset {
        sort: SortKey::Added,
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &wanted).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\nsort = \"added\"\n"
    );
}

#[test]
fn entries_that_stay_keep_their_order_and_new_ones_follow_sorted() {
    let fixture = Fixture::with("[presets.P]\ntags = [\"zed\", \"alpha\", \"mid\"]\n");
    let wanted = Preset {
        tags: tags(&["zed", "mid", "beta", "aardvark"]),
        ..Preset::default()
    };
    assert!(set_preset(&fixture.paths, "P", &wanted).unwrap());
    assert_eq!(
        fixture.read(),
        "schema_version = 1\n[presets.P]\ntags = [\"zed\", \"mid\", \"aardvark\", \"beta\"]\n"
    );
}

#[test]
fn tags_with_quotes_and_backslashes_are_escaped_when_joined() {
    for existing in ["[\"a\"]", "['a']", "[\"a\", 'b']"] {
        let fixture = Fixture::with(&format!("[presets.P]\ntags = {existing}\n"));
        let wanted = Preset {
            tags: tags(&["a", "it's \"x\"", "back\\slash", "tab\there"]),
            ..Preset::default()
        };
        assert!(
            set_preset(&fixture.paths, "P", &wanted).unwrap(),
            "{existing}"
        );
        let read = fixture.preset("P").tags;
        assert!(read.contains("it's \"x\""), "{existing}");
        assert!(read.contains("back\\slash"), "{existing}");
        assert!(read.contains("tab\there"), "{existing}");
        assert!(read.contains("a"), "{existing}");
    }
}

#[test]
fn renaming_an_inline_preset_keeps_its_comment_and_place() {
    let text = "[presets]\n# first\nAlpha = { sort = \"added\" }\n# the one to rename\nOld = { sort = \"playtime\", tags = [\"a\"] } # trailing\n# last\nZed = { sort = \"tags\" }\n";
    let fixture = Fixture::with(text);
    assert!(rename_preset(&fixture.paths, "Old", "Middle").unwrap());
    assert_eq!(
        fixture.read(),
        format!(
            "schema_version = 1\n{}",
            text.replace("Old = ", "Middle = ")
        )
    );
    assert_eq!(fixture.presets(), ["Alpha", "Middle", "Zed"]);
    assert_eq!(fixture.preset("Middle").sort, SortKey::Playtime);
}

#[test]
fn renaming_a_dotted_preset_keeps_its_comment_and_place() {
    let text = "schema_version = 1\n# first\npresets.Alpha.sort = \"added\"\n# the one to rename\npresets.Old.sort = \"playtime\"\npresets.Old.tags = [\"a\"] # trailing\n# last\npresets.Zed.sort = \"tags\"\n";
    let fixture = Fixture::with(text);
    assert_eq!(fixture.presets(), ["Alpha", "Old", "Zed"]);
    assert!(rename_preset(&fixture.paths, "Old", "Middle").unwrap());
    assert_eq!(
        fixture.read(),
        text.replace("presets.Old.", "presets.Middle.")
    );
    assert_eq!(fixture.presets(), ["Alpha", "Middle", "Zed"]);
    assert_eq!(fixture.preset("Middle").sort, SortKey::Playtime);
}

#[test]
fn renaming_a_standard_preset_keeps_the_comment_above_its_header() {
    let text = "# first\n[presets.Alpha]\nsort = \"added\"\n\n# the one to rename\n[presets.Old]\nsort = \"playtime\"\n\n# last\n[presets.Zed]\nsort = \"tags\"\n";
    let fixture = Fixture::with(text);
    assert!(rename_preset(&fixture.paths, "Old", "Middle").unwrap());
    assert_eq!(
        fixture.read(),
        format!(
            "schema_version = 1\n{}",
            text.replace("[presets.Old]", "[presets.Middle]")
        )
    );
}

#[test]
fn renaming_over_an_entry_that_is_not_a_preset_is_refused() {
    let text = "[presets]\nOld = { sort = \"added\" }\nNew = 3\nAfter = { sort = \"tags\" }\n";
    let fixture = Fixture::with(text);
    assert!(matches!(
        rename_preset(&fixture.paths, "Old", "New"),
        Err(Error::PresetNameTaken(taken)) if taken == "New"
    ));
    assert_eq!(fixture.read(), text);
}

/// Pairs that `to_lowercase` keeps apart but that are the same name to a
/// reader: the two sigmas, and sharp s against SS.
const FOLDED: [(&str, &str); 3] = [
    ("ΟΔΥΣΣΕΥΣ", "οδυσσευς"),
    ("Σίσυφος", "ςίσυφοσ"),
    ("Straße", "STRASSE"),
];

#[test]
fn names_that_differ_only_in_unicode_case_collide_on_save_and_rename() {
    for (first, second) in FOLDED {
        let fixture = Fixture::new();
        set_preset(&fixture.paths, first, &couch()).unwrap();
        let error = set_preset(&fixture.paths, second, &Preset::default()).unwrap_err();
        assert!(
            matches!(&error, Error::PresetNameTaken(taken) if taken == first),
            "{first} / {second}: {error:?}"
        );
        assert_eq!(
            fixture.library().taken_preset_name(second),
            Some(first),
            "{first} / {second}"
        );
        assert_eq!(fixture.preset(first), couch());

        set_preset(&fixture.paths, "Other", &couch()).unwrap();
        let error = rename_preset(&fixture.paths, "Other", second).unwrap_err();
        assert!(
            matches!(&error, Error::PresetNameTaken(taken) if taken == first),
            "{first} / {second}"
        );
        // Its own name in another case is still fine.
        assert!(rename_preset(&fixture.paths, first, second).unwrap());
    }
}

#[test]
fn presets_sort_ignoring_unicode_case_too() {
    // Plain lowercasing keeps the two sigmas apart and sorts "ςb" first.
    let fixture =
        Fixture::with("[presets.\"ςb\"]\n[presets.\"σa\"]\n[presets.\"Ä\"]\n[presets.\"z\"]\n");
    assert_eq!(fixture.presets(), ["z", "Ä", "σa", "ςb"]);
}

#[test]
fn several_entries_added_to_an_empty_or_lone_list_are_spaced() {
    for existing in ["[]", "[\"a\"]"] {
        let fixture = Fixture::with(&format!("[presets.P]\ntags = {existing}\n"));
        let wanted = Preset {
            tags: tags(&["c", "d", "e"]),
            ..Preset::default()
        };
        assert!(set_preset(&fixture.paths, "P", &wanted).unwrap());
        let text = fixture.read();
        assert!(text.contains("[\"c\", \"d\", \"e\"]"), "{existing}: {text}");
    }
    let fixture = Fixture::with("[presets.P]\ntags = [\"a\"]\n");
    let wanted = Preset {
        tags: tags(&["a", "b", "c"]),
        ..Preset::default()
    };
    set_preset(&fixture.paths, "P", &wanted).unwrap();
    assert!(fixture.read().contains("[\"a\", \"b\", \"c\"]"));
    // A list already written without spaces keeps that style.
    let fixture = Fixture::with("[presets.P]\ntags = [\"a\",\"b\"]\n");
    let wanted = Preset {
        tags: tags(&["a", "b", "c"]),
        ..Preset::default()
    };
    set_preset(&fixture.paths, "P", &wanted).unwrap();
    assert!(fixture.read().contains("[\"a\",\"b\",\"c\"]"));
}

#[test]
fn create_preset_saves_a_new_name() {
    let fixture = Fixture::new();
    assert!(create_preset(&fixture.paths, "Couch co-op", &couch()).unwrap());
    assert_eq!(fixture.preset("Couch co-op"), couch());
}

#[test]
fn create_preset_refuses_a_name_taken_exactly_or_in_another_case_and_writes_nothing() {
    let fixture = Fixture::with(
        "schema_version = 1\n\n# Kept.\n[presets.\"Couch co-op\"]\ntags = [\"vr\"]\n",
    );
    let before = fixture.read();
    let inode = fixture.inode();
    for name in ["Couch co-op", "couch CO-OP"] {
        let error = create_preset(&fixture.paths, name, &couch()).unwrap_err();
        assert!(
            matches!(&error, Error::PresetNameTaken(taken) if taken == "Couch co-op"),
            "{name}: {error:?}"
        );
        assert_eq!(error.to_string(), "The name Couch co-op is taken.");
    }
    assert_eq!(fixture.read(), before);
    assert_eq!(fixture.inode(), inode);
    // The upsert still saves over the exact name.
    assert!(set_preset(&fixture.paths, "Couch co-op", &couch()).unwrap());
}

#[test]
fn create_preset_checks_the_name_like_set_preset() {
    let fixture = Fixture::new();
    assert!(matches!(
        create_preset(&fixture.paths, " padded", &couch()),
        Err(Error::InvalidPresetName(_))
    ));
    assert!(!fixture.paths.library_file().exists());
}

const NON_PRESETS: &str =
    "[presets]\nbroken = 1\nlist = [\"a\", \"b\"]\n\n[presets.Old]\nsort = \"added\"\n";

#[test]
fn saving_refuses_a_name_a_non_preset_entry_already_uses() {
    for (name, taken) in [
        ("broken", "broken"),
        ("Broken", "broken"),
        ("list", "list"),
        ("LIST", "list"),
    ] {
        let fixture = Fixture::with(NON_PRESETS);
        let inode = fixture.inode();
        for error in [
            set_preset(&fixture.paths, name, &couch()).unwrap_err(),
            create_preset(&fixture.paths, name, &couch()).unwrap_err(),
        ] {
            assert!(
                matches!(&error, Error::PresetNameTaken(found) if found == taken),
                "{name}: {error:?}"
            );
        }
        assert_eq!(fixture.read(), NON_PRESETS, "{name}");
        assert_eq!(fixture.inode(), inode, "{name}");
    }
}

#[test]
fn renaming_refuses_a_name_a_non_preset_entry_already_uses() {
    for (new, taken) in [
        ("broken", "broken"),
        ("BROKEN", "broken"),
        ("list", "list"),
        ("List", "list"),
    ] {
        let fixture = Fixture::with(NON_PRESETS);
        let error = rename_preset(&fixture.paths, "Old", new).unwrap_err();
        assert!(
            matches!(&error, Error::PresetNameTaken(found) if found == taken),
            "{new}: {error:?}"
        );
        assert_eq!(fixture.read(), NON_PRESETS, "{new}");
    }
}

#[test]
fn a_non_preset_entry_folds_case_like_a_preset_in_every_write() {
    let text = "[presets]\n\"ςb\" = 1\n\n[presets.Old]\nsort = \"added\"\n";
    for name in ["ΣB", "σb", "ςB"] {
        let fixture = Fixture::with(text);
        let inode = fixture.inode();
        for error in [
            set_preset(&fixture.paths, name, &couch()).unwrap_err(),
            create_preset(&fixture.paths, name, &couch()).unwrap_err(),
            rename_preset(&fixture.paths, "Old", name).unwrap_err(),
        ] {
            assert!(
                matches!(&error, Error::PresetNameTaken(found) if found == "ςb"),
                "{name}: {error:?}"
            );
        }
        assert_eq!(fixture.read(), text, "{name}");
        assert_eq!(fixture.inode(), inode, "{name}");
        assert_eq!(fixture.library().taken_preset_name(name), Some("ςb"));
    }
}

#[test]
fn the_library_reports_the_keys_that_are_not_presets() {
    let fixture = Fixture::with(NON_PRESETS);
    let library = fixture.library();
    assert_eq!(library.other_presets, ["broken", "list"]);
    assert_eq!(library.taken_preset_name("BROKEN"), Some("broken"));
    assert_eq!(library.taken_preset_name("old"), Some("Old"));
}

#[test]
fn preset_name_taken_prefers_the_exact_name_and_skips_the_exception() {
    let names = ["A", "a", "b"];
    assert_eq!(preset_name_taken(names, "a", None), Some("a"));
    assert_eq!(preset_name_taken(names, "A", None), Some("A"));
    assert_eq!(preset_name_taken(names, "B", None), Some("b"));
    assert_eq!(preset_name_taken(names, "c", None), None);
    assert_eq!(preset_name_taken(names, "B", Some("b")), None);
    assert_eq!(preset_name_taken(names, "a", Some("a")), Some("A"));
}

#[test]
fn a_non_preset_entry_survives_next_to_a_save_and_a_rename() {
    let fixture = Fixture::with(NON_PRESETS);
    assert!(set_preset(&fixture.paths, "Fresh", &couch()).unwrap());
    assert!(rename_preset(&fixture.paths, "Old", "Newer").unwrap());
    let text = fixture.read();
    assert!(text.contains("broken = 1"), "{text}");
    assert!(text.contains("list = [\"a\", \"b\"]"), "{text}");
    assert_eq!(fixture.presets(), ["Fresh", "Newer"]);
}
