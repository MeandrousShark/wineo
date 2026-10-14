use super::*;

fn parse(s: &str) -> DocumentMut {
    s.parse().expect("valid toml")
}

#[test]
fn global_steam_sets_and_unsets_without_losing_comments() {
    let mut doc = parse("# mine\nsteam = \"native\" # preferred\nstop_timeout = 3 # short\n");
    set(&mut doc, Scope::Global, "steam", "flatpak").unwrap();
    assert_eq!(
        doc.to_string(),
        "# mine\nsteam = \"flatpak\" # preferred\nstop_timeout = 3 # short\n"
    );
    let config: crate::config::GlobalConfig = toml::from_str(&doc.to_string()).unwrap();
    assert_eq!(config.steam, Some(crate::steam::InstallKind::Flatpak));
    set(&mut doc, Scope::Global, "steam", "native").unwrap();
    let config: crate::config::GlobalConfig = toml::from_str(&doc.to_string()).unwrap();
    assert_eq!(config.steam, Some(crate::steam::InstallKind::Native));
    assert!(unset(&mut doc, Scope::Global, "steam").unwrap());
    assert!(doc.get("steam").is_none());
    assert!(doc.to_string().contains("stop_timeout = 3 # short"));
    let before = doc.to_string();
    assert!(!unset(&mut doc, Scope::Global, "steam").unwrap());
    assert_eq!(doc.to_string(), before);
}

#[test]
fn global_steam_rejects_invalid_values_without_changing_the_document() {
    for original in ["", "steam = 'flatpak' # preferred\n"] {
        for value in ["snap", "", "Flatpak"] {
            let mut doc = parse(original);
            assert!(matches!(
                set(&mut doc, Scope::Global, "steam", value),
                Err(EditError::InvalidValue { key, .. }) if key == "steam"
            ));
            assert_eq!(doc.to_string(), original);
        }
    }
}

#[test]
fn global_steam_no_op_preserves_string_formatting() {
    let original = "# mine\nsteam  =  'flatpak' # preferred\n";
    let mut doc = parse(original);
    set(&mut doc, Scope::Global, "steam", "flatpak").unwrap();
    assert_eq!(doc.to_string(), original);
}

#[test]
fn global_art_fetch_writes_custom_and_empty_arrays() {
    let mut doc = "[art]\nfetch = \"all\" # keep\n"
        .parse::<DocumentMut>()
        .unwrap();
    set(&mut doc, Scope::Global, "art.fetch", "grid,logo").unwrap();
    let config: crate::config::GlobalConfig = toml::from_str(&doc.to_string()).unwrap();
    assert_eq!(
        config.art.fetch.kinds(),
        vec![crate::art::Kind::Grid, crate::art::Kind::Logo]
    );
    assert!(doc.to_string().contains("# keep"));
    set(&mut doc, Scope::Global, "art.fetch", "none").unwrap();
    let config: crate::config::GlobalConfig = toml::from_str(&doc.to_string()).unwrap();
    assert!(config.art.fetch.kinds().is_empty());
    assert!(set(&mut doc, Scope::Global, "art.fetch", "grid,grid").is_err());
}

#[test]
fn global_art_fetch_sets_and_unsets_without_losing_comments() {
    let mut doc = parse("# art choice\n[art]\nfetch = \"all\" # selected\n");
    set(&mut doc, Scope::Global, "art.fetch", "icon").unwrap();
    assert_eq!(
        doc.to_string(),
        "# art choice\n[art]\nfetch = \"icon\" # selected\n"
    );
    assert!(unset(&mut doc, Scope::Global, "art.fetch").unwrap());
    assert!(!doc.to_string().contains("fetch"));
    assert!(set(&mut doc, Scope::Global, "art.fetch", "wrong").is_err());
}

// -- parse_assignment --

#[test]
fn parse_assignment_splits_at_first_equals() {
    assert_eq!(parse_assignment("key=value").unwrap(), ("key", "value"));
}

#[test]
fn parse_assignment_keeps_extra_equals_in_the_value() {
    assert_eq!(parse_assignment("key=a=b").unwrap(), ("key", "a=b"));
}

#[test]
fn parse_assignment_requires_an_equals() {
    let err = parse_assignment("no-equals-here").unwrap_err();
    assert!(matches!(err, EditError::MissingEquals(s) if s == "no-equals-here"));
}

// -- set: top-level keys --

#[test]
fn set_top_level_key_in_empty_doc() {
    let mut doc = parse("");
    set(&mut doc, Scope::Environment, "prefix", "/home/x/prefix").unwrap();
    assert_eq!(doc.to_string(), "prefix = \"/home/x/prefix\"\n");
}

#[test]
fn set_top_level_key_renders_before_existing_tables() {
    let mut doc = parse("[env]\nFOO = \"1\"\n");
    set(&mut doc, Scope::Environment, "runner", "wine").unwrap();
    // `[env]` keeps its own original spacing (none, since it was the
    // first line); only the new key is inserted ahead of it.
    assert_eq!(doc.to_string(), "runner = \"wine\"\n[env]\nFOO = \"1\"\n");
}

#[test]
fn set_updates_existing_key_and_keeps_other_comments() {
    let mut doc = parse("# top comment\nprefix = \"/old\"\nrunner = \"wine\" # keep me\n");
    set(&mut doc, Scope::Environment, "prefix", "/new").unwrap();
    assert_eq!(
        doc.to_string(),
        "# top comment\nprefix = \"/new\"\nrunner = \"wine\" # keep me\n"
    );
}

// -- set: creating tables --

#[test]
fn set_creates_env_table() {
    let mut doc = parse("prefix = \"/x\"\n");
    set(&mut doc, Scope::Environment, "env.WINEDEBUG", "fixme-all").unwrap();
    assert_eq!(
        doc.to_string(),
        "prefix = \"/x\"\n\n[env]\nWINEDEBUG = \"fixme-all\"\n"
    );
}

#[test]
fn set_creates_target_with_implicit_parent() {
    let mut doc = parse("");
    set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.exe",
        "bridge.exe",
    )
    .unwrap();
    assert_eq!(doc.to_string(), "[targets.bridge]\nexe = \"bridge.exe\"\n");
}

#[test]
fn set_whole_target_key_is_invalid() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Environment, "targets.bridge", "x").unwrap_err();
    assert!(matches!(
        err,
        EditError::InvalidValue { key, .. } if key == "targets.bridge"
    ));
}

// -- set: string formatting --

#[test]
fn set_string_with_backslash_renders_as_literal() {
    let mut doc = parse("");
    set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.exe",
        "bridge.exe",
    )
    .unwrap();
    set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.cwd",
        "C:\\Program Files\\Bridge",
    )
    .unwrap();
    assert_eq!(
        doc.to_string(),
        "[targets.bridge]\nexe = \"bridge.exe\"\ncwd = 'C:\\Program Files\\Bridge'\n"
    );
}

// -- set: args arrays --

#[test]
fn set_args_array_of_strings() {
    let mut doc = parse("");
    set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.args",
        r#"["-a", "-b"]"#,
    )
    .unwrap();
    assert_eq!(
        doc.to_string(),
        "[targets.bridge]\nargs = [\"-a\", \"-b\"]\n"
    );
}

#[test]
fn set_args_rejects_non_array() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Environment, "targets.bridge.args", "5").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.args"));
    // the failed parse must not have left a stray empty table behind
    assert_eq!(doc.to_string(), "");
}

#[test]
fn set_args_rejects_non_string_item() {
    let mut doc = parse("");
    let err = set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.args",
        r#"["-a", 5]"#,
    )
    .unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.args"));
}

#[test]
fn tags_set_and_unset_preserve_other_comments() {
    let mut doc = parse("# mine\nkind = 'native' # keep\n");
    set(&mut doc, Scope::Environment, "tags", r#"["game", "co-op"]"#).unwrap();
    assert_eq!(
        doc.to_string(),
        "# mine\nkind = 'native' # keep\ntags = [\"game\", \"co-op\"]\n"
    );
    assert!(unset(&mut doc, Scope::Environment, "tags").unwrap());
    assert_eq!(doc.to_string(), "# mine\nkind = 'native' # keep\n");
    assert!(!unset(&mut doc, Scope::Environment, "tags").unwrap());
    set(&mut doc, Scope::Environment, "tags", "[]").unwrap();
    assert!(doc.to_string().contains("tags = []\n"));
}

#[test]
fn winetricks_set_and_unset_preserve_comments() {
    let mut doc = parse("# mine\nprefix = '/p' # keep\n");
    set(
        &mut doc,
        Scope::Environment,
        "winetricks",
        r#"["win10", "corefonts"]"#,
    )
    .unwrap();
    assert!(
        doc.to_string()
            .contains("prefix = '/p' # keep\nwinetricks = [\"win10\", \"corefonts\"]")
    );
    for bad in ["[1]", "['-q']", "['a', 'a']"] {
        assert!(set(&mut doc, Scope::Environment, "winetricks", bad).is_err());
    }
    assert!(unset(&mut doc, Scope::Environment, "winetricks").unwrap());
    assert_eq!(doc.to_string(), "# mine\nprefix = '/p' # keep\n");
}

#[test]
fn tags_set_rejects_invalid_values_without_editing() {
    let mut doc = parse("kind = 'native'\n");
    for (value, expected) in [
        ("5", "expected an array of strings"),
        ("[5]", "array items must be strings"),
        ("['co op']", "Invalid tag \"co op\": spaces not allowed."),
        ("['game', 'game']", "Duplicate tag \"game\"."),
    ] {
        let err = set(&mut doc, Scope::Environment, "tags", value).unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
        assert_eq!(doc.to_string(), "kind = 'native'\n");
    }
}

// -- set: bool parsing --

#[test]
fn set_default_rejects_non_bool() {
    let mut doc = parse("");
    let err = set(
        &mut doc,
        Scope::Environment,
        "targets.bridge.default",
        "maybe",
    )
    .unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bridge.default"));
}

// -- set: stop_timeout --

#[test]
fn set_stop_timeout_valid() {
    let mut doc = parse("");
    set(&mut doc, Scope::Global, "stop_timeout", "10").unwrap();
    assert_eq!(doc.to_string(), "stop_timeout = 10\n");
}

#[test]
fn set_stop_timeout_rejects_negative() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Global, "stop_timeout", "-1").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "stop_timeout"));
}

#[test]
fn set_stop_timeout_rejects_non_number() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Global, "stop_timeout", "soon").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "stop_timeout"));
}

// -- set: defaults.arch --

#[test]
fn set_defaults_arch_valid() {
    let mut doc = parse("");
    set(&mut doc, Scope::Global, "defaults.arch", "win64").unwrap();
    assert_eq!(doc.to_string(), "[defaults]\narch = \"win64\"\n");
}

#[test]
fn set_defaults_arch_rejects_unknown_arch() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Global, "defaults.arch", "win95").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "defaults.arch"));
}

// -- set: default target switching --

#[test]
fn set_default_switches_between_targets() {
    let mut doc = parse("");
    set(&mut doc, Scope::Environment, "targets.a.exe", "a.exe").unwrap();
    set(&mut doc, Scope::Environment, "targets.a.default", "true").unwrap();
    set(&mut doc, Scope::Environment, "targets.b.exe", "b.exe").unwrap();
    set(&mut doc, Scope::Environment, "targets.b.default", "true").unwrap();
    assert_eq!(
        doc.to_string(),
        "[targets.a]\nexe = \"a.exe\"\n\n[targets.b]\nexe = \"b.exe\"\ndefault = true\n"
    );
}

#[test]
fn set_default_false_never_creates_targets() {
    let mut doc = parse("[targets.a]\nexe = \"a.exe\"\ndefault = true\n");
    set(&mut doc, Scope::Environment, "targets.a.default", "false").unwrap();
    set(
        &mut doc,
        Scope::Environment,
        "targets.ghost.default",
        "false",
    )
    .unwrap();
    assert_eq!(doc.to_string(), "[targets.a]\nexe = \"a.exe\"\n");
}

// -- unset --

#[test]
fn unset_env_var_removes_empty_table() {
    let mut doc = parse("[env]\nFOO = \"1\"\n");
    let removed = unset(&mut doc, Scope::Environment, "env.FOO").unwrap();
    assert!(removed);
    assert_eq!(doc.to_string(), "");
}

#[test]
fn unset_whole_target() {
    let mut doc =
        parse("[targets.bridge]\nexe = \"bridge.exe\"\n\n[targets.other]\nexe = \"other.exe\"\n");
    let removed = unset(&mut doc, Scope::Environment, "targets.bridge").unwrap();
    assert!(removed);
    // `[targets.other]` keeps the blank line it was originally written
    // with, even though it's now the first table in the document.
    assert_eq!(doc.to_string(), "\n[targets.other]\nexe = \"other.exe\"\n");
}

#[test]
fn unset_missing_key_returns_false() {
    let mut doc = parse("");
    assert!(!unset(&mut doc, Scope::Environment, "prefix").unwrap());
    assert!(!unset(&mut doc, Scope::Environment, "env.FOO").unwrap());
    assert!(!unset(&mut doc, Scope::Environment, "targets.bridge").unwrap());
    assert!(!unset(&mut doc, Scope::Environment, "targets.bridge.exe").unwrap());
}

// -- unknown keys --

#[test]
fn unknown_key_env_scope() {
    let mut doc = parse("");
    assert!(matches!(
        set(&mut doc, Scope::Environment, "envv.X", "1"),
        Err(EditError::UnknownKey(k)) if k == "envv.X"
    ));
    assert!(matches!(
        unset(&mut doc, Scope::Environment, "envv.X"),
        Err(EditError::UnknownKey(k)) if k == "envv.X"
    ));
    assert!(matches!(
        set(&mut doc, Scope::Environment, "targets.a.bogus", "1"),
        Err(EditError::UnknownKey(k)) if k == "targets.a.bogus"
    ));
    assert!(matches!(
        set(&mut doc, Scope::Environment, "stop_timeout", "1"),
        Err(EditError::UnknownKey(k)) if k == "stop_timeout"
    ));
}

#[test]
fn unknown_key_global_scope() {
    let mut doc = parse("");
    assert!(matches!(
        set(&mut doc, Scope::Global, "prefix", "/x"),
        Err(EditError::UnknownKey(k)) if k == "prefix"
    ));
    assert!(matches!(
        set(&mut doc, Scope::Global, "envv.X", "1"),
        Err(EditError::UnknownKey(k)) if k == "envv.X"
    ));
}

// -- read-only arch --

#[test]
fn arch_is_read_only() {
    let mut doc = parse("");
    assert!(matches!(
        set(&mut doc, Scope::Environment, "arch", "win64"),
        Err(EditError::ReadOnly { key, .. }) if key == "arch"
    ));
    assert!(matches!(
        unset(&mut doc, Scope::Environment, "arch"),
        Err(EditError::ReadOnly { key, .. }) if key == "arch"
    ));
}

// -- invalid names --

#[test]
fn set_rejects_invalid_target_name() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Environment, "targets.bad name.exe", "x").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "targets.bad name.exe"));
}

#[test]
fn set_rejects_invalid_env_var_name() {
    let mut doc = parse("");
    let err = set(&mut doc, Scope::Environment, "env.FOO=BAR", "x").unwrap_err();
    assert!(matches!(err, EditError::InvalidValue { key, .. } if key == "env.FOO=BAR"));
}

#[test]
fn launch_bools_work_at_every_level() {
    let mut global = parse("");
    let mut env = parse("");
    for key in ["wayland", "mangohud"] {
        set(&mut global, Scope::Global, key, "true").unwrap();
        assert!(unset(&mut global, Scope::Global, key).unwrap());
        set(&mut env, Scope::Environment, key, "false").unwrap();
        assert!(unset(&mut env, Scope::Environment, key).unwrap());
        let target = format!("targets.game.{key}");
        set(&mut env, Scope::Environment, &target, "true").unwrap();
        assert!(unset(&mut env, Scope::Environment, &target).unwrap());
        assert!(set(&mut global, Scope::Global, key, "yes").is_err());
    }
    assert!(matches!(
        set(&mut global, Scope::Global, "gamescope", "false"),
        Err(EditError::UnknownKey(_))
    ));
}

#[test]
fn gamescope_fields_are_typed_and_unsetting_the_last_one_keeps_the_table() {
    let fields = [
        ("width", "1920"),
        ("height", "1080"),
        ("output_width", "2560"),
        ("output_height", "1440"),
        ("fps_limit", "60"),
        ("fps_limit_unfocused", "0"),
        ("filter", "fsr"),
        ("scaler", "fit"),
        ("window", "borderless"),
        ("grab_cursor", "true"),
        ("args", r#"["--hdr-enabled"]"#),
    ];
    for prefix in ["gamescope", "targets.game.gamescope"] {
        let mut doc = if prefix.starts_with("targets") {
            parse("[targets.game]\nexe = 'game'\ngamescope = false # old\n")
        } else {
            parse("gamescope = false # old\n")
        };
        for (field, raw) in fields {
            let key = format!("{prefix}.{field}");
            set(&mut doc, Scope::Environment, &key, raw).unwrap();
            assert!(doc.to_string().contains(&format!("{field} = ")));
            assert!(doc.to_string().contains("# old"), "{doc}");
            assert!(unset(&mut doc, Scope::Environment, &key).unwrap());
            assert!(doc.to_string().contains(&format!("[{prefix}]")), "{doc}");
        }
        assert!(!unset(&mut doc, Scope::Environment, &format!("{prefix}.width")).unwrap());
        assert!(unset(&mut doc, Scope::Environment, prefix).unwrap());
        assert!(!doc.to_string().contains(&format!("[{prefix}]")), "{doc}");
        set(&mut doc, Scope::Environment, prefix, "false").unwrap();
        assert!(doc.to_string().contains("gamescope = false"), "{doc}");
        let err = set(&mut doc, Scope::Environment, prefix, "true").unwrap_err();
        assert!(
            err.to_string()
                .contains("gamescope takes a [gamescope] table or false, not true")
        );
        assert!(doc.to_string().contains("gamescope = false"), "{doc}");
    }
}

#[test]
fn an_empty_table_turns_gamescope_on_with_no_fields() {
    for prefix in ["gamescope", "targets.game.gamescope"] {
        let start = if prefix.starts_with("targets") {
            "[targets.game]\nexe = 'game'\ngamescope = false # old\n"
        } else {
            "gamescope = false # old\n"
        };
        let mut doc = parse(start);
        set(&mut doc, Scope::Environment, prefix, "{}").unwrap();
        assert!(doc.to_string().contains(&format!("[{prefix}]")), "{doc}");
        assert!(doc.to_string().contains("# old"), "{doc}");
        assert!(!doc.to_string().contains("gamescope = false"), "{doc}");
        // An existing table is emptied rather than merged into.
        set(
            &mut doc,
            Scope::Environment,
            &format!("{prefix}.width"),
            "1920",
        )
        .unwrap();
        set(&mut doc, Scope::Environment, prefix, "{}").unwrap();
        assert!(!doc.to_string().contains("width"), "{doc}");
        assert!(doc.to_string().contains(&format!("[{prefix}]")), "{doc}");
        let config = crate::config::EnvironmentConfig::parse(
            std::path::Path::new("/x/game.toml"),
            &format!("prefix = '~/p'\nrunner = 'wine'\narch = 'win64'\n{doc}"),
        )
        .unwrap();
        let setting = if prefix.starts_with("targets") {
            config.targets["game"].gamescope.clone()
        } else {
            config.gamescope.clone()
        };
        assert_eq!(
            setting,
            Some(crate::gamescope::GamescopeSetting::On(Default::default()))
        );
    }
    let mut doc = parse("");
    assert!(set(&mut doc, Scope::Environment, "gamescope", "{width = 1}").is_err());
    assert_eq!(doc.to_string(), "");
}

#[test]
fn every_gamescope_choice_is_accepted() {
    use crate::gamescope::{Filter, Scaler, Window};
    let lists: [(&str, Vec<&str>); 3] = [
        ("filter", Filter::ALL.map(Filter::as_str).to_vec()),
        ("scaler", Scaler::ALL.map(Scaler::as_str).to_vec()),
        ("window", Window::ALL.map(Window::as_str).to_vec()),
    ];
    for (field, values) in lists {
        let key = format!("gamescope.{field}");
        for value in &values {
            let mut doc = parse("");
            set(&mut doc, Scope::Environment, &key, value).unwrap();
            let config = crate::config::EnvironmentConfig::parse(
                std::path::Path::new("/x/game.toml"),
                &format!("prefix = '~/p'\nrunner = 'wine'\narch = 'win64'\n{doc}"),
            )
            .unwrap();
            let gamescope = config.gamescope.unwrap();
            let read = match field {
                "filter" => gamescope.on().unwrap().filter.map(Filter::as_str),
                "scaler" => gamescope.on().unwrap().scaler.map(Scaler::as_str),
                _ => gamescope.on().unwrap().window.map(Window::as_str),
            };
            assert_eq!(read, Some(*value));
        }
        // Pinned, so a value dropped from a list shows up here.
        let accepted: &[&str] = match field {
            "filter" => &["linear", "nearest", "fsr", "nis", "pixel", "sgsr"],
            "scaler" => &["auto", "integer", "fit", "fill", "stretch"],
            _ => &["fullscreen", "borderless"],
        };
        assert_eq!(values, accepted);
        let mut doc = parse("");
        assert!(set(&mut doc, Scope::Environment, &key, "bilinear").is_err());
    }
}

#[test]
fn invalid_gamescope_fields_do_not_change_the_document() {
    let mut doc = parse("gamescope = false\n");
    for (field, value) in [
        ("width", "-1"),
        ("height", "4294967296"),
        ("filter", "bilinear"),
        ("scaler", "zoom"),
        ("window", "windowed"),
        ("grab_cursor", "yes"),
        ("args", "[1]"),
        ("unknown", "2"),
    ] {
        assert!(
            set(
                &mut doc,
                Scope::Environment,
                &format!("gamescope.{field}"),
                value
            )
            .is_err()
        );
        assert_eq!(doc.to_string(), "gamescope = false\n");
    }
}

#[test]
fn hooks_edits_keep_comments_and_empty_override_tables() {
    for prefix in ["hooks", "targets.game.hooks"] {
        let mut doc = parse(&format!(
            "[targets.game]\nexe = 'game' # target\n\n[{prefix}] # hooks\nbefore = 'mount /old' # before\nafter = 'sync' # after\n"
        ));
        set(
            &mut doc,
            Scope::Environment,
            &format!("{prefix}.before"),
            "mount /mnt/games",
        )
        .unwrap();
        let text = doc.to_string();
        assert!(
            text.contains("before = \"mount /mnt/games\" # before"),
            "{text}"
        );
        assert!(text.contains("after = 'sync' # after"), "{text}");
        assert!(text.contains("exe = 'game' # target"), "{text}");
        assert!(unset(&mut doc, Scope::Environment, &format!("{prefix}.after")).unwrap());
        assert!(!unset(&mut doc, Scope::Environment, &format!("{prefix}.after")).unwrap());
        assert!(unset(&mut doc, Scope::Environment, &format!("{prefix}.before")).unwrap());
        assert!(
            doc.to_string().contains(&format!("[{prefix}] # hooks")),
            "{doc}"
        );
        set(&mut doc, Scope::Environment, prefix, "false").unwrap();
        set(
            &mut doc,
            Scope::Environment,
            &format!("{prefix}.after"),
            "notify-send done",
        )
        .unwrap();
        let config = crate::config::EnvironmentConfig::parse(
            std::path::Path::new("/x.toml"),
            &format!("kind = 'native'\n{doc}"),
        )
        .unwrap();
        let setting = if prefix.starts_with("targets") {
            &config.targets["game"].hooks
        } else {
            &config.hooks
        };
        assert_eq!(
            setting.as_ref().unwrap().on().unwrap().after.as_deref(),
            Some("notify-send done")
        );
        set(&mut doc, Scope::Environment, prefix, "{}").unwrap();
        assert!(!doc.to_string().contains("after ="), "{doc}");
        assert!(unset(&mut doc, Scope::Environment, prefix).unwrap());
    }
}

#[test]
fn invalid_hooks_edits_leave_the_document_unchanged() {
    for (key, value) in [
        ("hooks", "true"),
        ("hooks", "{before = 'sync'}"),
        ("hooks.before", ""),
        ("hooks.after", ""),
        ("hooks.unknown", "sync"),
        ("targets.new.hooks", "true"),
        ("targets.new.hooks.before", ""),
        ("targets.new.hooks.unknown", "sync"),
    ] {
        let mut doc = parse("hooks = false # mine\n");
        let before = doc.to_string();
        assert!(set(&mut doc, Scope::Environment, key, value).is_err());
        assert_eq!(doc.to_string(), before);
    }
    let mut doc = parse("");
    assert!(set(&mut doc, Scope::Global, "hooks.before", "sync").is_err());
    assert!(unset(&mut doc, Scope::Global, "hooks").is_err());
    assert!(unset(&mut doc, Scope::Environment, "hooks.unknown").is_err());
    assert!(unset(&mut doc, Scope::Environment, "targets.game.hooks.unknown").is_err());
    assert_eq!(doc.to_string(), "");
}
