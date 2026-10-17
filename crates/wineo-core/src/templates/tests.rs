use super::*;

fn templates(root: &Path) -> Templates {
    Templates::at(root.join("templates"))
}

fn write(root: &Path, name: &str, text: &str) {
    let dir = root.join("templates");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.toml")), text).unwrap();
}

#[test]
fn the_built_in_names_no_version() {
    let root = tempfile::tempdir().unwrap();
    let gaming = templates(root.path()).resolve("gaming").unwrap();
    assert_eq!(gaming.dxvk.as_deref(), Some("latest"));
    assert_eq!(gaming.vkd3d.as_deref(), Some("latest"));
    assert_eq!(gaming.gamemode, Some(true));
    assert_eq!(gaming.runner.as_deref(), Some(NEWEST_PROTON));
    assert_eq!(gaming.tags, ["game"]);

    let mut doc: DocumentMut = "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n"
        .parse()
        .unwrap();
    gaming.apply(&mut doc);
    let config =
        config::EnvironmentConfig::parse(Path::new("/game.toml"), &doc.to_string()).unwrap();
    assert_eq!(config.tags, ["game"]);
}

#[test]
fn a_file_shadows_a_built_in_of_the_same_name() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "gaming",
        "tags = ['work', 'co-op']\ngamemode = false\n",
    );
    let gaming = templates(root.path()).resolve("gaming").unwrap();
    assert_eq!(gaming.gamemode, Some(false));
    assert_eq!(gaming.dxvk, None);
    assert_eq!(gaming.tags, ["work", "co-op"]);

    let mut doc: DocumentMut = "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n"
        .parse()
        .unwrap();
    gaming.apply(&mut doc);
    let config =
        config::EnvironmentConfig::parse(Path::new("/game.toml"), &doc.to_string()).unwrap();
    assert_eq!(config.tags, ["work", "co-op"]);

    let listed = templates(root.path()).list().unwrap();
    assert_eq!(
        listed,
        [Listed {
            name: "gaming".to_owned(),
            built_in: false
        }]
    );
}

#[test]
fn listing_covers_built_ins_and_files() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        templates(root.path()).list().unwrap(),
        [Listed {
            name: "gaming".to_owned(),
            built_in: true
        }]
    );

    write(root.path(), "hades", "runner = \"wine-tkg\"\n");
    write(root.path(), "notes", "not a template");
    fs::write(root.path().join("templates/README"), "").unwrap();
    let listed = templates(root.path()).list().unwrap();
    let names: Vec<&str> = listed.iter().map(|row| row.name.as_str()).collect();
    // Listing does not parse, so a broken file still shows up.
    assert_eq!(names, ["gaming", "hades", "notes"]);
}

#[test]
fn a_template_parses_everything_an_environment_may_set() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "full",
        "runner = \"wine-tkg\"\narch = \"win32\"\ngamemode = true\ndxvk = \"2.7\"\nvkd3d = \"2.14\"\n\n[env]\nDXVK_HUD = \"fps\"\n",
    );
    let full = templates(root.path()).resolve("full").unwrap();
    assert_eq!(full.runner.as_deref(), Some("wine-tkg"));
    assert_eq!(full.arch, Some(Arch::Win32));
    assert_eq!(full.dxvk.as_deref(), Some("2.7"));
    assert_eq!(full.env["DXVK_HUD"], "fps");
}

#[test]
fn template_tags_are_validated_and_applied() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "good", "tags = ['game', 'co-op']\n");
    let template = templates(root.path()).resolve("good").unwrap();
    assert_eq!(template.tags, ["game", "co-op"]);
    let mut doc: DocumentMut = "kind = 'native'\n".parse().unwrap();
    template.apply(&mut doc);
    let config =
        config::EnvironmentConfig::parse(Path::new("/native.toml"), &doc.to_string()).unwrap();
    assert_eq!(config.tags, ["game", "co-op"]);

    for tags in ["['co op']", "['game', 'game']"] {
        write(root.path(), "bad", &format!("tags = {tags}\n"));
        assert!(matches!(
            templates(root.path()).resolve("bad"),
            Err(Error::ConfigInvalid { .. })
        ));
    }
}

#[test]
fn template_winetricks_are_validated_and_applied() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "verbs",
        "winetricks = ['vcrun2022', 'corefonts']\n",
    );
    let template = templates(root.path()).resolve("verbs").unwrap();
    let mut doc: DocumentMut = "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n"
        .parse()
        .unwrap();
    template.apply(&mut doc);
    let config =
        config::EnvironmentConfig::parse(Path::new("/wine.toml"), &doc.to_string()).unwrap();
    assert_eq!(config.winetricks, ["vcrun2022", "corefonts"]);
    for verbs in ["['a', 'a']", "['-q']", "[1]"] {
        write(root.path(), "bad", &format!("winetricks = {verbs}\n"));
        assert!(templates(root.path()).resolve("bad").is_err(), "{verbs}");
    }
}

#[test]
fn per_environment_keys_are_refused_by_name() {
    let root = tempfile::tempdir().unwrap();
    for key in ["prefix = \"/p\"", "[targets.app]\nexe = \"a\""] {
        write(root.path(), "bad", key);
        let err = templates(root.path()).resolve("bad").unwrap_err();
        assert!(
            err.to_string().contains("belongs to an environment"),
            "{key}: {err}"
        );
    }
}

#[test]
fn a_template_may_carry_the_schema_version() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "v1", "schema_version = 1\ngamemode = true\n");
    write(root.path(), "legacy", "gamemode = true\n");
    let v1 = templates(root.path()).resolve("v1").unwrap();
    assert_eq!(v1, templates(root.path()).resolve("legacy").unwrap());
    assert_eq!(v1.gamemode, Some(true));
}

#[test]
fn reading_a_template_leaves_it_alone() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "legacy", "# mine\ngamemode = true\n");
    let path = root.path().join("templates/legacy.toml");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    templates(root.path()).resolve("legacy").unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# mine\ngamemode = true\n"
    );
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn unsupported_template_versions_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    for value in ["0", "2", "\"1\""] {
        write(root.path(), "t", &format!("schema_version = {value}\n"));
        assert!(
            matches!(
                templates(root.path()).resolve("t"),
                Err(Error::SchemaVersion { .. })
            ),
            "{value}"
        );
    }
}

#[test]
fn an_unknown_key_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "typo", "gamemod = true\n");
    let err = templates(root.path()).resolve("typo").unwrap_err();
    assert!(matches!(err, Error::ConfigUnknownKey { .. }), "{err}");
    let message = err.to_string();
    assert!(message.contains("unknown field `gamemod`"), "{message}");
    assert!(
        message.ends_with(
            "\nA newer version of wineo may have written it; update wineo or remove the key."
        ),
        "{message}"
    );
}

#[test]
fn template_syntax_and_type_errors_are_not_unknown_keys() {
    let root = tempfile::tempdir().unwrap();
    for text in ["gamemode = [\n", "gamemode = 3\n"] {
        write(root.path(), "t", text);
        let err = templates(root.path()).resolve("t").unwrap_err();
        assert!(matches!(err, Error::ConfigSyntax { .. }), "{text}: {err}");
        assert!(!err.to_string().contains("A newer version"), "{err}");
    }
}

#[test]
fn a_newer_template_version_gets_the_version_hint() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "t", "schema_version = 2\n");
    let err = templates(root.path()).resolve("t").unwrap_err();
    assert!(
        err.to_string()
            .ends_with("\nA newer version of wineo wrote it; update wineo to use this file."),
        "{err}"
    );
}

#[test]
fn an_unknown_template_says_what_is_available() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "hades", "");
    let err = templates(root.path()).resolve("nope").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("gaming"), "{message}");
    assert!(message.contains("hades"), "{message}");

    assert!(matches!(
        templates(root.path()).resolve("bad name"),
        Err(Error::InvalidName(_))
    ));
}

#[test]
fn applying_writes_only_what_the_template_sets() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "t",
        "runner = \"wine-tkg\"\narch = \"win32\"\ndxvk = \"latest\"\ngamemode = true\n\n[env]\nDXVK_HUD = \"fps\"\n",
    );
    let mut doc = DocumentMut::new();
    doc["prefix"] = value("~/p");
    templates(root.path()).resolve("t").unwrap().apply(&mut doc);
    let text = doc.to_string();

    assert!(text.contains("dxvk = \"latest\""), "{text}");
    assert!(text.contains("gamemode = true"), "{text}");
    assert!(text.contains("DXVK_HUD = \"fps\""), "{text}");
    // The caller already wrote these, having resolved them first.
    assert!(!text.contains("runner"), "{text}");
    assert!(!text.contains("arch"), "{text}");
    assert!(!text.contains("vkd3d"), "{text}");
}

#[test]
fn launch_settings_carry_into_the_environment() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "t",
        "wayland = true\nmangohud = false\n\n[gamescope]\nwidth = 1920\nwindow = \"fullscreen\"\n",
    );
    let template = templates(root.path()).resolve("t").unwrap();
    assert_eq!(template.wayland, Some(true));
    assert_eq!(template.mangohud, Some(false));
    let mut doc = crate::config::new_document();
    doc["prefix"] = value("/p");
    doc["runner"] = value("wine");
    doc["arch"] = value("win64");
    template.apply(&mut doc);
    let text = doc.to_string();
    assert!(text.contains("wayland = true\n"), "{text}");
    assert!(text.contains("mangohud = false\n"), "{text}");
    assert!(
        text.contains("[gamescope]\nwidth = 1920\nwindow = \"fullscreen\"\n"),
        "{text}"
    );
    let config = crate::config::EnvironmentConfig::parse(Path::new("/t.toml"), &text).unwrap();
    assert_eq!(config.gamescope, template.gamescope);

    write(root.path(), "off", "gamescope = false\n");
    let off = templates(root.path()).resolve("off").unwrap();
    let mut doc = DocumentMut::new();
    off.apply(&mut doc);
    assert_eq!(doc.to_string(), "gamescope = false\n");

    write(root.path(), "on", "gamescope = true\n");
    assert!(templates(root.path()).resolve("on").is_err());
}

#[test]
fn hooks_carry_into_new_environment_documents() {
    let root = tempfile::tempdir().unwrap();
    for hooks in [
        "[hooks]\nbefore = 'mount /mnt/games'\nafter = 'sync'\n",
        "[hooks]\n",
        "hooks = false\n",
    ] {
        write(root.path(), "hooks", hooks);
        let path = root.path().join("templates/hooks.toml");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let template = templates(root.path()).resolve("hooks").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), hooks);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        for base in [
            "prefix = '/p'\nrunner = 'wine'\narch = 'win64'\n",
            "kind = 'native'\n",
        ] {
            let mut doc: DocumentMut = base.parse().unwrap();
            template.apply(&mut doc);
            let config =
                crate::config::EnvironmentConfig::parse(Path::new("/t.toml"), &doc.to_string())
                    .unwrap();
            assert_eq!(config.hooks, template.hooks);
        }
    }
}

#[test]
fn templates_reject_invalid_hooks() {
    let root = tempfile::tempdir().unwrap();
    for text in [
        "hooks = true\n",
        "[hooks]\nbefore = ''\n",
        "[hooks]\nafter = ''\n",
        "[hooks]\nbefore = 1\n",
        "[hooks]\nunknown = 'sync'\n",
    ] {
        write(root.path(), "bad", text);
        assert!(templates(root.path()).resolve("bad").is_err(), "{text}");
    }
}

/// Templates always deserialized from the parsed table, so that is what the
/// pass has to match.
#[test]
fn a_file_without_deprecated_keys_errors_exactly_as_before() {
    let path = Path::new("/templates/coop.toml");
    for text in [
        "tags = \n",
        "gamemode = 'yes'\n",
        "[env]\nFOO = 1\n",
        "gamemod = true\n",
    ] {
        let new = crate::error::chain(&parse(path, text).unwrap_err());
        let plain = match toml::from_str::<toml::Table>(text) {
            Err(source) => config::deserialize_error(path, source),
            Ok(table) => config::deserialize_error(path, table.try_into::<Template>().unwrap_err()),
        };
        assert_eq!(new, crate::error::chain(&plain), "{text}");
    }
}
