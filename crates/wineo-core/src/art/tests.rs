use super::*;
use crate::error::Error;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";
const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

/// One 16x16 image whose 4 data bytes follow the directory.
fn ico() -> Vec<u8> {
    let mut bytes = vec![0, 0, 1, 0, 1, 0];
    bytes.extend([16, 16, 0, 0, 1, 0, 32, 0]);
    bytes.extend(4u32.to_le_bytes());
    bytes.extend(22u32.to_le_bytes());
    bytes.extend([1, 2, 3, 4]);
    bytes
}

#[test]
fn magic_bytes_detect_raster_formats() {
    assert_eq!(detect(PNG), Some(Format::Png));
    assert_eq!(detect(JPEG), Some(Format::Jpeg));
    assert_eq!(detect(WEBP), Some(Format::Webp));
    assert_eq!(detect(&ico()), Some(Format::Ico));
    assert_eq!(detect(b"RIFF\x24\0\0\0WAVEfmt "), None);
    assert_eq!(detect(b"RIFF"), None);
    assert_eq!(detect(b""), None);
}

#[test]
fn a_renamed_text_file_is_refused() {
    assert_eq!(detect(b"just some notes\n"), None);
    assert_eq!(detect(b"<html><body></body></html>"), None);
}

#[test]
fn a_weak_or_truncated_ico_is_refused() {
    let full = ico();
    assert_eq!(detect(&full[..full.len() - 1]), None);
    assert_eq!(detect(&full[..10]), None);
    assert_eq!(detect(&full[..6]), None);
    let mut empty = full.clone();
    empty[4] = 0;
    assert_eq!(detect(&empty), None);
    let mut cursor = full.clone();
    cursor[2] = 2;
    assert_eq!(detect(&cursor), None);
    let mut many = full.clone();
    many[4] = 200;
    assert_eq!(detect(&many), None);
    let mut inside = full;
    inside[18..22].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(detect(&inside), None);
}

#[test]
fn svg_is_sniffed_past_its_prolog() {
    for text in [
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        "<svg>",
        "\u{feff}<svg/>",
        "\n\t <?xml version=\"1.0\"?>\n<svg width=\"1\">",
        "<?xml version=\"1.0\"?><!-- made by <hand> --><svg\n>",
        "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x>y.dtd\">\n<svg>",
        "<!DOCTYPE svg [ <!ENTITY a \"<b>\"> ]><!-- c --><svg>",
    ] {
        assert_eq!(detect(text.as_bytes()), Some(Format::Svg), "{text}");
    }
}

#[test]
fn xml_without_an_svg_root_is_refused() {
    for text in [
        "<?xml version=\"1.0\"?><html><svg/></html>",
        "<svgx>",
        "<svg",
        "<!-- <svg> -->",
        "<!-- never closed <svg>",
        "<?xml never closed <svg>",
        "<!DOCTYPE svg [ <svg>",
        "text <svg>",
    ] {
        assert_eq!(detect(text.as_bytes()), None, "{text}");
    }
}

#[test]
fn each_kind_accepts_its_formats() {
    for kind in Kind::ALL {
        for format in [Format::Png, Format::Svg, Format::Jpeg] {
            assert!(kind.accepts(format), "{kind} {format}");
        }
        assert_eq!(kind.accepts(Format::Ico), kind == Kind::Icon, "{kind}");
        assert_eq!(kind.accepts(Format::Webp), kind != Kind::Icon, "{kind}");
    }
}

#[test]
fn kinds_parse_and_name_their_files() {
    assert_eq!("hero".parse::<Kind>(), Ok(Kind::Hero));
    assert!("banner".parse::<Kind>().is_err());
    assert_eq!(Kind::Grid.file_name(Format::Jpeg), "grid.jpg");
    assert_eq!(Kind::Icon.file_name(Format::Ico), "icon.ico");
    assert_eq!(Kind::Logo.file_name(Format::Webp), "logo.webp");
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, b"x").unwrap();
    path
}

#[test]
fn missing_art_finds_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(find(root.path(), Kind::Icon, None).unwrap(), None);
    let absent = root.path().join("absent");
    assert_eq!(find(&absent, Kind::Icon, Some(Format::Png)).unwrap(), None);
}

#[test]
fn unrecorded_duplicates_resolve_in_order() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let webp = touch(dir, "icon.webp");
    let jpg = touch(dir, "icon.jpg");
    let ico = touch(dir, "icon.ico");
    let svg = touch(dir, "icon.svg");
    let png = touch(dir, "icon.png");
    let found = find(dir, Kind::Icon, None).unwrap().unwrap();
    // WebP is not an icon format, so it is not art at all.
    assert_eq!(
        found,
        Found {
            path: png.clone(),
            format: Format::Png,
            duplicates: vec![svg.clone(), ico.clone(), jpg.clone()],
        }
    );
    fs::remove_file(&png).unwrap();
    fs::remove_file(&svg).unwrap();
    let found = find(dir, Kind::Icon, None).unwrap().unwrap();
    assert_eq!((found.format, found.duplicates), (Format::Ico, vec![jpg]));
    assert!(webp.exists());
}

#[test]
fn the_recorded_format_wins_when_its_file_exists() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let png = touch(dir, "grid.png");
    let webp = touch(dir, "grid.webp");
    let found = find(dir, Kind::Grid, Some(Format::Webp)).unwrap().unwrap();
    assert_eq!(
        (found.path, found.duplicates),
        (webp.clone(), vec![png.clone()])
    );
    let found = find(dir, Kind::Grid, Some(Format::Jpeg)).unwrap().unwrap();
    assert_eq!((found.path, found.duplicates), (png, vec![webp]));
}

#[test]
fn directories_named_like_art_are_not_art() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("logo.png")).unwrap();
    assert_eq!(find(root.path(), Kind::Logo, None).unwrap(), None);
}

#[test]
fn a_target_falls_back_to_its_environment() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    let env_icon = touch(art, "icon.png");
    let env_hero = touch(art, "hero.jpg");
    let target_icon = touch(&target_dir(art, "editor"), "icon.svg");
    let config = ArtConfig::default();
    let at = |target, kind| {
        resolve(art, &config, target, kind)
            .unwrap()
            .map(|r| (r.origin, r.found.path))
    };

    assert_eq!(
        at(Some("editor"), Kind::Icon),
        Some((Origin::Target, target_icon))
    );
    assert_eq!(
        at(Some("editor"), Kind::Hero),
        Some((Origin::Environment, env_hero))
    );
    assert_eq!(
        at(None, Kind::Icon),
        Some((Origin::Environment, env_icon.clone()))
    );
    assert_eq!(
        at(Some("other"), Kind::Icon),
        Some((Origin::Environment, env_icon))
    );
    assert_eq!(at(Some("editor"), Kind::Logo), None);
}

const EXAMPLE: &str = "schema_version = 1
game = 5247

[icon]
file = \"icon.png\"
steamgriddb = 36952

[grid]
file = \"grid.jpg\"
manual = \"poster final.jpg\"

[targets.server.icon]
file = \"icon.svg\"
manual = \"server.svg\"
";

fn example() -> ArtConfig {
    ArtConfig::parse(Path::new("art.toml"), EXAMPLE).unwrap()
}

fn manual(format: Format, original: &str) -> Record {
    Record {
        format,
        source: Source::Manual(original.to_owned()),
    }
}

#[test]
fn the_plan_example_parses() {
    let config = example();
    assert_eq!(config.schema_version, 1);
    assert_eq!(config.game, Some(5247));
    assert_eq!(
        config.record(None, Kind::Icon),
        Some(&Record {
            format: Format::Png,
            source: Source::SteamGridDb(36952),
        })
    );
    assert_eq!(
        config.record(None, Kind::Grid),
        Some(&manual(Format::Jpeg, "poster final.jpg"))
    );
    assert_eq!(config.record(None, Kind::Hero), None);
    assert_eq!(
        config.record(Some("server"), Kind::Icon),
        Some(&manual(Format::Svg, "server.svg"))
    );
    assert_eq!(config.record(Some("server"), Kind::Grid), None);
    assert_eq!(config.record(Some("other"), Kind::Icon), None);
}

fn read(art: &Path) -> String {
    fs::read_to_string(art.join(ART_FILE)).unwrap()
}

#[test]
fn a_new_file_matches_the_plan_example() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path().join("hades");
    let mut file = ArtFile::open(&art).unwrap();
    assert!(!file.save().unwrap());
    assert!(!art.exists());

    file.set_game(Some(5247)).unwrap();
    let icon = Record {
        format: Format::Png,
        source: Source::SteamGridDb(36952),
    };
    file.set(None, Kind::Icon, icon).unwrap();
    file.set(None, Kind::Grid, manual(Format::Jpeg, "poster final.jpg"))
        .unwrap();
    file.set(
        Some("server"),
        Kind::Icon,
        manual(Format::Svg, "server.svg"),
    )
    .unwrap();
    assert_eq!(
        file.config(),
        &ArtConfig {
            schema_version: 0,
            ..example()
        }
    );
    assert!(file.save().unwrap());
    assert_eq!(read(&art), EXAMPLE);
    assert_eq!(ArtConfig::load(&art).unwrap(), example());
    assert!(!ArtFile::open(&art).unwrap().save().unwrap());
}

#[test]
fn comments_survive_a_set_and_a_forget() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    let original = "# Art for hades
schema_version = 1

[icon] # the official one
file = \"icon.png\" # keep this
steamgriddb = 36952

[targets.server.logo]
file = \"logo.png\"
manual = \"logo.png\"
";
    fs::write(art.join(ART_FILE), original).unwrap();
    let mut file = ArtFile::open(art).unwrap();
    file.set(None, Kind::Icon, manual(Format::Png, "mine.png"))
        .unwrap();
    file.set(None, Kind::Hero, manual(Format::Jpeg, "wide.jpg"))
        .unwrap();
    assert!(file.remove(Some("server"), Kind::Logo));
    assert!(!file.remove(Some("server"), Kind::Logo));
    assert!(!file.remove(None, Kind::Grid));
    assert!(file.save().unwrap());
    let text = read(art);
    assert!(text.starts_with("# Art for hades\n"), "{text}");
    assert!(text.contains("[icon] # the official one\n"), "{text}");
    assert!(text.contains("file = \"icon.png\" # keep this\n"), "{text}");
    assert!(text.contains("schema_version = 1\n"), "{text}");
    assert!(!text.contains("steamgriddb"), "{text}");
    assert!(!text.contains("targets"), "{text}");

    let config = ArtConfig::load(art).unwrap();
    assert_eq!(
        config.record(None, Kind::Icon),
        Some(&manual(Format::Png, "mine.png"))
    );
    assert_eq!(
        config.record(None, Kind::Hero),
        Some(&manual(Format::Jpeg, "wide.jpg"))
    );
    assert!(config.targets.is_empty());

    let mut file = ArtFile::open(art).unwrap();
    assert!(file.remove(None, Kind::Hero));
    assert!(file.save().unwrap());
    let text = read(art);
    assert!(text.starts_with("# Art for hades\n"), "{text}");
    assert!(text.contains("# keep this"), "{text}");
    assert!(!text.contains("hero"), "{text}");
}

#[test]
fn inline_tables_can_be_edited() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    fs::write(
        art.join(ART_FILE),
        "schema_version = 1\nicon = { file = \"icon.png\", manual = \"a.png\" }\n\
             targets = { server = { grid = { file = \"grid.png\", manual = \"b.png\" } } }\n",
    )
    .unwrap();
    let mut file = ArtFile::open(art).unwrap();
    let asset = |format| Record {
        format,
        source: Source::SteamGridDb(7),
    };
    file.set(None, Kind::Icon, asset(Format::Ico)).unwrap();
    file.set(Some("server"), Kind::Logo, asset(Format::Webp))
        .unwrap();
    file.save().unwrap();
    let config = ArtConfig::load(art).unwrap();
    assert_eq!(config.record(None, Kind::Icon), Some(&asset(Format::Ico)));
    assert_eq!(
        config.record(Some("server"), Kind::Logo),
        Some(&asset(Format::Webp))
    );
    assert!(config.record(Some("server"), Kind::Grid).is_some());
}

#[test]
fn an_edited_legacy_file_gets_the_version() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    fs::write(art.join(ART_FILE), "game = 1\n").unwrap();
    let mut file = ArtFile::open(art).unwrap();
    assert!(!file.save().unwrap());
    assert_eq!(read(art), "game = 1\n");
    file.set_game(Some(2)).unwrap();
    file.save().unwrap();
    assert_eq!(read(art), "game = 2\nschema_version = 1\n");
    assert_eq!(file.config().schema_version, 1);
}

#[test]
fn an_absent_art_toml_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(ArtConfig::load(root.path()).unwrap(), ArtConfig::default());
    assert_eq!(
        ArtConfig::load(&root.path().join("absent")).unwrap(),
        ArtConfig::default()
    );
}

#[test]
fn a_missing_schema_version_is_legacy() {
    let text = "[logo]\nfile = \"logo.png\"\nsteamgriddb = 1\n";
    let config = ArtConfig::parse(Path::new("art.toml"), text).unwrap();
    assert_eq!(config.schema_version, 0);
    assert!(config.record(None, Kind::Logo).is_some());
}

#[test]
fn unsupported_schema_versions_are_refused() {
    for version in ["0", "2", "\"1\"", "1.0"] {
        let text = format!("schema_version = {version}\n");
        let err = ArtConfig::parse(Path::new("art.toml"), &text).unwrap_err();
        assert!(
            matches!(err, Error::SchemaVersion { .. }),
            "{version}: {err}"
        );
    }
}

#[test]
fn unknown_or_missing_keys_are_refused() {
    for text in [
        "schema_version = 1\ncolor = 1\n",
        "[icon]\nfile = \"icon.png\"\nmanual = \"a\"\nscore = 3\n",
        "[banner]\nfile = \"banner.png\"\nmanual = \"a\"\n",
        "[targets.server.banner]\nfile = \"banner.png\"\nmanual = \"a\"\n",
    ] {
        let err = ArtConfig::parse(Path::new("art.toml"), text).unwrap_err();
        assert!(
            matches!(err, Error::ConfigUnknownKey { .. }),
            "{text}: {err}"
        );
    }
    for text in ["[icon]\nmanual = \"a\"\n", "game = -1\n"] {
        let err = ArtConfig::parse(Path::new("art.toml"), text).unwrap_err();
        assert!(matches!(err, Error::ConfigSyntax { .. }), "{text}: {err}");
    }
}

#[test]
fn a_gog_source_is_read_written_and_replaces_the_others() {
    let text = "[grid]\nfile = \"grid.jpg\"\ngog = 1094900565\n";
    let config = ArtConfig::parse(Path::new("art.toml"), text).unwrap();
    assert_eq!(
        config.record(None, Kind::Grid).map(|record| &record.source),
        Some(&Source::Gog(1_094_900_565))
    );

    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(ART_FILE),
        "[grid]\nfile = \"grid.jpg\"\nmanual = \"a.jpg\"\n",
    )
    .unwrap();
    let mut file = ArtFile::open(root.path()).unwrap();
    let record = Record {
        format: Format::Jpeg,
        source: Source::Gog(7),
    };
    file.set(None, Kind::Grid, record.clone()).unwrap();
    file.save().unwrap();
    let written = fs::read_to_string(root.path().join(ART_FILE)).unwrap();
    assert!(
        written.contains("gog = 7") && !written.contains("manual"),
        "{written}"
    );
    let reread = ArtConfig::load(root.path()).unwrap();
    assert_eq!(reread.record(None, Kind::Grid), Some(&record));
}

#[test]
fn a_kind_needs_exactly_one_source_and_a_valid_file() {
    for text in [
        "[icon]\nfile = \"icon.png\"\n",
        "[icon]\nfile = \"icon.png\"\nsteamgriddb = 1\nmanual = \"a\"\n",
        "[icon]\nfile = \"icon.png\"\ngog = 1\nmanual = \"a\"\n",
        "[icon]\nfile = \"icon.png\"\ngog = 1\nsteamgriddb = 1\n",
        "[targets.server.hero]\nfile = \"hero.png\"\n",
        "[icon]\nfile = \"icon.webp\"\nmanual = \"a\"\n",
        "[icon]\nfile = \"grid.png\"\nmanual = \"a\"\n",
        "[icon]\nfile = \"../icon.png\"\nmanual = \"a\"\n",
        "[grid]\nfile = \"grid.jpeg\"\nmanual = \"a\"\n",
        "[targets.\"bad.name\".icon]\nfile = \"icon.png\"\nmanual = \"a\"\n",
    ] {
        let err = ArtConfig::parse(Path::new("art.toml"), text).unwrap_err();
        assert!(matches!(err, Error::ConfigInvalid { .. }), "{text}: {err}");
    }
}

#[test]
fn recorded_files_win_and_others_are_unrecorded() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    let png = touch(art, "grid.png");
    let jpg = touch(art, "grid.jpg");
    let config = example();

    let grid = resolve(art, &config, None, Kind::Grid).unwrap().unwrap();
    assert_eq!(grid.found.path, jpg);
    assert_eq!(grid.found.duplicates, vec![png.clone()]);
    assert_eq!(grid.source, Some(Source::Manual("poster final.jpg".into())));

    // The recorded file is gone, so what is left is unrecorded.
    fs::remove_file(&jpg).unwrap();
    let grid = resolve(art, &config, None, Kind::Grid).unwrap().unwrap();
    assert_eq!((grid.found.path, grid.source), (png, None));

    let hero = touch(art, "hero.webp");
    let resolved = resolve(art, &config, None, Kind::Hero).unwrap().unwrap();
    assert_eq!((resolved.found.path, resolved.source), (hero, None));
    let resolved = resolve(art, &ArtConfig::default(), None, Kind::Grid)
        .unwrap()
        .unwrap();
    assert_eq!(resolved.source, None);
}

#[test]
fn each_dir_uses_its_own_record() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    let env_png = touch(art, "icon.png");
    let target = target_dir(art, "server");
    touch(&target, "icon.png");
    let svg = touch(&target, "icon.svg");
    let config = example();

    let icon = resolve(art, &config, Some("server"), Kind::Icon)
        .unwrap()
        .unwrap();
    assert_eq!(icon.origin, Origin::Target);
    assert_eq!(icon.found.path, svg);
    assert_eq!(icon.source, Some(Source::Manual("server.svg".into())));

    let icon = resolve(art, &config, None, Kind::Icon).unwrap().unwrap();
    assert_eq!(icon.found.path, env_png);
    assert_eq!(icon.source, Some(Source::SteamGridDb(36952)));

    // The target records its SVG, so its leftover PNG is unrecorded.
    fs::remove_file(&svg).unwrap();
    let icon = resolve(art, &config, Some("server"), Kind::Icon)
        .unwrap()
        .unwrap();
    assert_eq!((icon.origin, icon.source), (Origin::Target, None));
}

fn store() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| {
        (key == "HOME").then(|| std::ffi::OsString::from(home.as_os_str()))
    })
    .unwrap();
    let config = paths.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n\
             [targets.server]\nexe = \"server.exe\"\n",
    )
    .unwrap();
    (root, paths)
}

fn source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join("from").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn manual_art_rechecks_identity_after_waiting_for_art_lock() {
    let (root, paths) = store();
    let prior = source(root.path(), "prior.png", PNG);
    set_manual(&paths, "hades", None, Kind::Grid, &prior, None).unwrap();
    let identity = EnvironmentIdentity::read(&paths, "hades").unwrap();
    let replacement = source(root.path(), "replacement.jpg", JPEG);
    let art_dir = paths.art_dir("hades");
    let saved_art = fs::read(art_dir.join("grid.png")).unwrap();
    let saved_record = fs::read(art_dir.join(ART_FILE)).unwrap();
    let held = Locks::new(&paths).config(&art_dir.join(ART_FILE)).unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let paths = paths.clone();
        move || {
            started.send(()).unwrap();
            set_manual(
                &paths,
                "hades",
                None,
                Kind::Grid,
                &replacement,
                Some(identity),
            )
        }
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let config = paths.environment_config("hades");
    let next = config.with_extension("replacement");
    fs::write(&next, fs::read(&config).unwrap()).unwrap();
    fs::rename(next, config).unwrap();
    drop(held);

    let err = worker.join().unwrap().unwrap_err();
    assert_ne!(EnvironmentIdentity::read(&paths, "hades"), Some(identity));
    assert!(matches!(
        err,
        Error::ArtEnvironmentChanged(name) if name == "hades"
    ));
    assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), saved_art);
    assert_eq!(fs::read(art_dir.join(ART_FILE)).unwrap(), saved_record);
    assert!(!art_dir.join("grid.jpg").exists());
}

#[test]
fn set_as_records_the_given_name() {
    let (root, paths) = store();
    let file = source(root.path(), "0123abcd", WEBP);
    set_manual_as(&paths, "hades", None, Kind::Grid, &file, "heroic-a.webp").unwrap();
    let art = paths.art_dir("hades");
    assert_eq!(fs::read(art.join("grid.webp")).unwrap(), WEBP);
    assert_eq!(
        ArtConfig::load(&art).unwrap().record(None, Kind::Grid),
        Some(&manual(Format::Webp, "heroic-a.webp"))
    );
}

#[test]
fn set_keeps_the_bytes_and_records_the_original_name() {
    let (root, paths) = store();
    let file = source(root.path(), "My Poster.webp", WEBP);
    let stored = set_manual(&paths, "hades", None, Kind::Grid, &file, None).unwrap();
    let art = paths.art_dir("hades");
    assert_eq!(stored.path, art.join("grid.webp"));
    assert_eq!(stored.format, Format::Webp);
    assert_eq!(fs::read(&stored.path).unwrap(), WEBP);
    let text = read(&art);
    assert!(text.starts_with("schema_version = 1\n"), "{text}");
    assert_eq!(
        ArtConfig::load(&art).unwrap().record(None, Kind::Grid),
        Some(&manual(Format::Webp, "My Poster.webp"))
    );
    let listing = list(&paths, "hades").unwrap();
    let (_, grid) = &listing.kinds[1];
    assert_eq!(
        grid.as_ref().unwrap().source,
        Some(Source::Manual("My Poster.webp".into()))
    );
}

#[test]
fn set_refuses_by_content_kind_and_size() {
    let (root, paths) = store();
    let text = source(root.path(), "icon.png", b"not an image");
    let icon = ico();
    let truncated = source(root.path(), "short.ico", &icon[..icon.len() - 1]);
    let svg = source(root.path(), "page.svg", b"<?xml version=\"1.0\"?><html/>");
    let webp = source(root.path(), "icon.webp", WEBP);
    for (file, kind) in [
        (&text, Kind::Icon),
        (&truncated, Kind::Icon),
        (&svg, Kind::Logo),
        (&webp, Kind::Icon),
    ] {
        let err = set_manual(&paths, "hades", None, kind, file, None).unwrap_err();
        assert!(matches!(err, Error::ArtFormat { .. }), "{err}");
    }
    let err = set_manual(
        &paths,
        "hades",
        None,
        Kind::Grid,
        &source(root.path(), "x.ico", &icon),
        None,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "{} is not a PNG, SVG, JPEG, or WebP image",
            root.path().join("from/x.ico").display()
        )
    );

    let mut big = PNG.to_vec();
    big.resize(MAX_BYTES as usize + 1, 0);
    let big = source(root.path(), "big.png", &big);
    let err = set_manual(&paths, "hades", None, Kind::Hero, &big, None).unwrap_err();
    assert!(matches!(err, Error::ArtTooLarge(_)), "{err}");
    let mut limit = PNG.to_vec();
    limit.resize(MAX_BYTES as usize, 0);
    let limit = source(root.path(), "limit.png", &limit);
    set_manual(&paths, "hades", None, Kind::Hero, &limit, None).unwrap();

    let ok = source(root.path(), "ok.png", PNG);
    let err = set_manual(&paths, "hades", Some("nope"), Kind::Icon, &ok, None).unwrap_err();
    assert!(matches!(err, Error::UnknownTarget { .. }), "{err}");
    let err = set_manual(&paths, "nope", None, Kind::Icon, &ok, None).unwrap_err();
    assert!(matches!(err, Error::UnknownEnvironment(_)), "{err}");
    let art = paths.art_dir("hades");
    assert!(!art.join("icon.png").exists());
    assert!(!art.join("grid.ico").exists());
}

#[test]
fn set_replaces_a_kind_that_changes_format() {
    let (root, paths) = store();
    let art = paths.art_dir("hades");
    let unrecorded = touch(&art, "icon.svg");
    let first = source(root.path(), "a.png", PNG);
    let stored = set_manual(&paths, "hades", None, Kind::Icon, &first, None).unwrap();
    assert_eq!(stored.removed, vec![unrecorded.clone()]);
    let second = source(root.path(), "b.ico", &ico());
    let stored = set_manual(&paths, "hades", None, Kind::Icon, &second, None).unwrap();
    assert_eq!(stored.path, art.join("icon.ico"));
    assert_eq!(stored.removed, vec![art.join("icon.png")]);
    let names: Vec<_> = fs::read_dir(&art)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"icon.ico".to_owned()));
}

#[test]
fn a_target_override_is_set_and_removed_on_its_own() {
    let (root, paths) = store();
    let art = paths.art_dir("hades");
    let env_icon = source(root.path(), "env.png", PNG);
    set_manual(&paths, "hades", None, Kind::Icon, &env_icon, None).unwrap();
    let svg = source(root.path(), "server.svg", b"<svg/>");
    let stored = set_manual(&paths, "hades", Some("server"), Kind::Icon, &svg, None).unwrap();
    assert_eq!(stored.path, art.join("server/icon.svg"));
    let listing = list(&paths, "hades").unwrap();
    assert_eq!(listing.targets.len(), 1);
    assert_eq!(listing.targets[0].1[0].1.found.path, stored.path);

    let removed = remove(&paths, "hades", Some("server"), Kind::Icon).unwrap();
    assert_eq!(removed.files, vec![stored.path]);
    assert!(removed.record);
    assert!(!art.join("server").exists());
    assert!(art.join("icon.png").exists());
    let config = ArtConfig::load(&art).unwrap();
    assert!(config.targets.is_empty());
    assert!(config.record(None, Kind::Icon).is_some());

    let err = remove(&paths, "hades", Some("server"), Kind::Icon).unwrap_err();
    assert_eq!(err.to_string(), "`hades.server` has no icon art");
    let err = remove(&paths, "hades", None, Kind::Logo).unwrap_err();
    assert_eq!(err.to_string(), "`hades` has no logo art");
}

#[test]
fn remove_takes_unrecorded_files_and_stale_records() {
    let (_root, paths) = store();
    let art = paths.art_dir("hades");
    let png = touch(&art, "hero.png");
    let jpg = touch(&art, "hero.jpg");
    let removed = remove(&paths, "hades", None, Kind::Hero).unwrap();
    assert_eq!((removed.files, removed.record), (vec![png, jpg], false));
    assert!(!art.join(ART_FILE).exists());

    fs::write(
        art.join(ART_FILE),
        "schema_version = 1\n[logo]\nfile = \"logo.png\"\nsteamgriddb = 3\n",
    )
    .unwrap();
    let removed = remove(&paths, "hades", None, Kind::Logo).unwrap();
    assert_eq!((removed.files.len(), removed.record), (0, true));
    assert_eq!(read(&art), "schema_version = 1\n");
}

#[test]
fn list_warns_without_changing_anything() {
    let (_root, paths) = store();
    let art = paths.art_dir("hades");
    let text = "schema_version = 1
[icon]
file = \"icon.svg\"
manual = \"a.svg\"

[targets.gone.logo]
file = \"logo.png\"
manual = \"b.png\"
";
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join(ART_FILE), text).unwrap();
    let png = touch(&art, "grid.png");
    let jpg = touch(&art, "grid.jpg");
    touch(&target_dir(&art, "gone"), "logo.png");

    let listing = list(&paths, "hades").unwrap();
    assert_eq!(
        listing.warnings,
        vec![
            Warning::MissingFile {
                target: None,
                kind: Kind::Icon,
                path: art.join("icon.svg"),
            },
            Warning::Duplicates {
                target: None,
                kind: Kind::Grid,
                used: png,
                ignored: vec![jpg],
                recorded: false,
            },
            Warning::UnknownTarget("gone".into()),
        ]
    );
    assert_eq!(listing.kinds[0], (Kind::Icon, None));
    assert!(listing.targets.is_empty());
    assert_eq!(read(&art), text);
}

#[test]
fn remove_all_takes_the_whole_dir() {
    let (root, paths) = store();
    assert!(!remove_all(&paths, "hades").unwrap());
    let file = source(root.path(), "a.png", PNG);
    set_manual(&paths, "hades", Some("server"), Kind::Icon, &file, None).unwrap();
    assert!(remove_all(&paths, "hades").unwrap());
    assert!(!paths.art_dir("hades").exists());
}

#[test]
fn staged_art_is_copied_and_never_replaces_anything() {
    let (root, paths) = store();
    let file = source(root.path(), "a.png", PNG);
    set_manual(&paths, "hades", Some("server"), Kind::Icon, &file, None).unwrap();
    set_manual(&paths, "hades", None, Kind::Grid, &file, None).unwrap();
    let locks = Locks::new(&paths);
    let source_dir = paths.art_dir("hades");
    let copy = paths.art_dir("copy");
    let staged = StagedArt::stage(&locks, &source_dir, &copy).unwrap();
    assert_eq!(staged.publish().unwrap(), Some(copy.clone()));
    for name in [ART_FILE, "grid.png", "server/icon.png"] {
        assert_eq!(
            fs::read(copy.join(name)).unwrap(),
            fs::read(source_dir.join(name)).unwrap(),
            "{name}"
        );
    }
    let staged = StagedArt::stage(&locks, &source_dir, &copy).unwrap();
    assert!(staged.publish().is_err());
    let art_root = copy.parent().unwrap();
    let mut names: Vec<_> = fs::read_dir(art_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["copy", "hades"]);

    let none = StagedArt::stage(&locks, &paths.art_dir("absent"), &paths.art_dir("x")).unwrap();
    assert_eq!(none.publish().unwrap(), None);
    assert!(!paths.art_dir("x").exists());
}

#[test]
fn a_removed_targets_art_is_cleaned_up() {
    let (root, paths) = store();
    let file = source(root.path(), "a.png", PNG);
    set_manual(&paths, "hades", None, Kind::Icon, &file, None).unwrap();
    set_manual(&paths, "hades", Some("server"), Kind::Icon, &file, None).unwrap();
    set_manual(&paths, "hades", Some("server"), Kind::Logo, &file, None).unwrap();
    let art = paths.art_dir("hades");
    touch(&target_dir(&art, "server"), "notes.txt");

    assert!(remove_target(&paths, "hades", "server").unwrap());
    assert!(!target_dir(&art, "server").exists());
    let config = ArtConfig::load(&art).unwrap();
    assert!(config.targets.is_empty());
    assert!(config.record(None, Kind::Icon).is_some());
    assert!(art.join("icon.png").exists());
    assert!(!remove_target(&paths, "hades", "server").unwrap());
    assert!(!remove_target(&paths, "other", "server").unwrap());
}

#[test]
fn an_edit_keeps_a_deprecated_spelling() {
    let root = tempfile::tempdir().unwrap();
    let art = root.path();
    let original = "schema_version = 1\ntest_old_game = 5 # mine\n\n[targets.main.test_old_icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n";
    fs::write(art.join(ART_FILE), original).unwrap();
    assert!(!ArtFile::open(art).unwrap().save().unwrap());
    assert_eq!(read(art), original);

    let mut file = ArtFile::open(art).unwrap();
    file.set_game(Some(6)).unwrap();
    file.set(Some("main"), Kind::Icon, manual(Format::Png, "b.png"))
        .unwrap();
    assert!(file.save().unwrap());
    assert_eq!(
        read(art),
        original
            .replace("= 5", "= 6")
            .replace("\"a.png\"", "\"b.png\"")
    );

    let mut file = ArtFile::open(art).unwrap();
    assert!(file.remove(Some("main"), Kind::Icon));
    assert!(file.save().unwrap());
    assert_eq!(read(art), "schema_version = 1\ntest_old_game = 6 # mine\n");
}
