use super::*;
use std::ffi::OsString;
use std::os::unix::fs::symlink;

/// An XDG root, an environment config, and a prefix holding its executable.
struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
    prefix: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let prefix = root.path().join("prefix");
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/Hades")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        fs::write(prefix.join("drive_c/Hades/Hades.exe"), b"").unwrap();
        Self {
            _root: root,
            paths,
            prefix,
        }
    }

    fn environment(&self, targets: &str) -> Environment {
        let path = self.paths.environment_config("hades");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = format!(
            "prefix = \"{}\"\nrunner = \"wine\"\narch = \"win64\"\n{targets}",
            self.prefix.display()
        );
        fs::write(&path, text).unwrap();
        Environment::load(&self.paths, "hades").unwrap()
    }

    fn proxied(&self, config: &str) -> Environment {
        let path = self.paths.environment_config("hades");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!("kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\n{config}"),
        )
        .unwrap();
        Environment::load(&self.paths, "hades").unwrap()
    }

    fn steam_game(&self, title: &str) -> PathBuf {
        let manifest = self
            ._root
            .path()
            .join(".local/share/Steam/steamapps/appmanifest_1086940.acf");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            &manifest,
            format!(
                "\"AppState\" {{ \"appid\" \"1086940\" \"name\" \"{title}\" \"StateFlags\" \"4\" }}"
            ),
        )
        .unwrap();
        manifest
    }

    fn reconcile_proxied(&self, environment: &Environment) -> Vec<Change> {
        reconcile_proxied(&self.paths, &GlobalConfig::default(), environment).unwrap()
    }

    /// Reconciles with no runner, so nothing tries to start wine for an icon.
    fn reconcile(&self, environment: &Environment) -> Vec<Change> {
        reconcile(
            &self.paths,
            &GlobalConfig::default(),
            environment,
            None,
            &self.prefix,
        )
        .unwrap()
    }

    fn entry(&self, target: &str) -> Option<String> {
        fs::read_to_string(entry_path(&self.paths, "hades", target)).ok()
    }

    /// Stores `bytes` as icon art for the environment, or `target`.
    fn art(&self, target: Option<&str>, file: &str, bytes: &[u8]) {
        let dir = art::spot_dir(&self.paths.art_dir("hades"), target);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), bytes).unwrap();
    }

    fn clear_art(&self, target: Option<&str>) {
        let dir = art::spot_dir(&self.paths.art_dir("hades"), target);
        for format in art::Format::ALL {
            let _ = fs::remove_file(dir.join(art::Kind::Icon.file_name(format)));
        }
    }

    fn icons(&self) -> Vec<String> {
        let hicolor = self.paths.icons_dir().join("hicolor");
        icon_files(&self.paths, "hades", "hades")
            .into_iter()
            .map(|path| path.strip_prefix(&hicolor).unwrap().display().to_string())
            .collect()
    }

    /// Stands in for what wine's menu builder would have written.
    fn exe_icon(&self, size: &str) {
        let icon = self
            .paths
            .icons_dir()
            .join("hicolor")
            .join(size)
            .join("apps/wineo-hades-hades.png");
        fs::create_dir_all(icon.parent().unwrap()).unwrap();
        fs::write(icon, b"png").unwrap();
    }
}

fn png(side: u32, shade: u8) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 0, 0, 255]));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

fn updated() -> Vec<Change> {
    vec![Change::Updated {
        target: "hades".to_owned(),
    }]
}

const ASKS: &str = "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n";

#[test]
fn writes_an_entry_for_a_target_that_asks() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    assert_eq!(
        fixture.reconcile(&environment),
        [Change::Wrote {
            target: "hades".to_owned(),
            icon: false,
        }]
    );
    assert_eq!(
        fixture.entry("hades").unwrap(),
        concat!(
            "[Desktop Entry]\n",
            "Type=Application\n",
            "Name=hades\n",
            "Comment=Runs in the wineo environment hades\n",
            "Exec=wineo start hades --target hades\n",
            "Terminal=false\n",
            "Categories=Utility;Software;\n",
            "StartupWMClass=Hades.exe\n",
            "X-Wineo-Environment=hades\n",
            "X-Wineo-Target=hades\n",
        )
    );
}

#[test]
fn a_portable_environment_never_touches_host_entries() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    assert_eq!(fixture.reconcile(&environment).len(), 1);
    let entry = fixture.entry("hades").unwrap();

    let usb = fixture._root.path().join("usb");
    let home = fixture._root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&usb),
    )
    .unwrap();
    let config = portable.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
            &config,
            "prefix = 'prefixes/hades'\nrunner = 'wine'\narch = 'win64'\n[targets.other]\nexe = 'notepad'\ndesktop = true\n",
        )
        .unwrap();
    let same_name = Environment::load(&portable, "hades").unwrap();
    let changes = reconcile(
        &portable,
        &GlobalConfig::default(),
        &same_name,
        None,
        &usb.join("prefixes/hades"),
    )
    .unwrap();
    assert_eq!(
        changes,
        [Change::Skipped {
            target: "other".to_owned(),
            reason: Error::PortableDesktop.to_string(),
        }]
    );
    assert!(remove_all(&portable, "hades").unwrap().is_empty());
    assert!(supported(&portable).is_err());
    assert_eq!(fixture.entry("hades").unwrap(), entry);
    assert_eq!(fixture.entry("other"), None);
}

// Reconciliation runs on every launch, so an entry that already matches has
// to cost nothing and rewrite nothing.
#[test]
fn leaves_a_matching_entry_alone() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    assert_eq!(fixture.reconcile(&environment).len(), 1);
    assert!(fixture.reconcile(&environment).is_empty());
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn a_target_with_no_setting_gets_no_entry() {
    let fixture = Fixture::new();
    let environment = fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n");
    assert!(fixture.reconcile(&environment).is_empty());
    assert_eq!(fixture.entry("hades"), None);
}

#[test]
fn clearing_the_setting_takes_the_entry_away() {
    let fixture = Fixture::new();
    let asking = fixture.environment(ASKS);
    fixture.reconcile(&asking);
    assert!(fixture.entry("hades").is_some());

    let no_longer = fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n");
    assert_eq!(
        fixture.reconcile(&no_longer),
        [Change::Removed {
            target: "hades".to_owned()
        }]
    );
    assert_eq!(fixture.entry("hades"), None);
}

#[test]
fn removing_the_target_takes_the_entry_away() {
    let fixture = Fixture::new();
    let asking = fixture.environment(ASKS);
    fixture.reconcile(&asking);
    let gone = fixture.environment("");
    assert_eq!(
        fixture.reconcile(&gone),
        [Change::Removed {
            target: "hades".to_owned()
        }]
    );
    assert_eq!(fixture.entry("hades"), None);
}

#[test]
fn the_name_field_titles_the_entry() {
    let fixture = Fixture::new();
    let environment = fixture.environment(
        "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\nname = \"Hades\"\n",
    );
    fixture.reconcile(&environment);
    assert!(fixture.entry("hades").unwrap().contains("\nName=Hades\n"));

    // Changing it rewrites the entry rather than adding a second one.
    let renamed = fixture.environment(
        "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\nname = \"Hades II\"\n",
    );
    assert_eq!(
        fixture.reconcile(&renamed),
        [Change::Updated {
            target: "hades".to_owned()
        }]
    );
    assert!(
        fixture
            .entry("hades")
            .unwrap()
            .contains("\nName=Hades II\n")
    );
}

#[test]
fn a_backslash_in_the_name_is_escaped_and_reads_back() {
    let fixture = Fixture::new();
    let environment = fixture.environment(
        "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\nname = 'AC\\DC'\n",
    );
    fixture.reconcile(&environment);
    assert!(
        fixture
            .entry("hades")
            .unwrap()
            .contains("\nName=AC\\\\DC\n")
    );
    assert_eq!(list(&fixture.paths).unwrap()[0].name, "AC\\DC");
    // The rendered text is what change detection compares, so it settles.
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn entry_escapes_read_back() {
    assert_eq!(unescape("AC\\\\DC\\sx\\q\\"), "AC\\DC x\\q\\");
}

#[test]
fn gamemode_makes_it_a_game() {
    let fixture = Fixture::new();
    let environment = fixture.environment(
        "gamemode = true\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n",
    );
    fixture.reconcile(&environment);
    assert!(
        fixture
            .entry("hades")
            .unwrap()
            .contains("\nCategories=Game;\n"),
        "{}",
        fixture.entry("hades").unwrap()
    );
}

#[test]
fn gamescope_and_mangohud_make_it_a_game() {
    for setting in ["mangohud = true\n", "[gamescope]\n"] {
        let fixture = Fixture::new();
        let environment = fixture.environment(&format!(
            "{setting}[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n"
        ));
        fixture.reconcile(&environment);
        assert!(
            fixture
                .entry("hades")
                .unwrap()
                .contains("\nCategories=Game;\n")
        );
    }
}

// One target with a bad exe must not stop the others, since reconciliation
// runs on every launch and a launch of `good` has nothing to do with it.
#[test]
fn skips_a_target_whose_exe_is_not_there() {
    let fixture = Fixture::new();
    let environment = fixture.environment(concat!(
        "\n[targets.typo]\nexe = 'C:\\Hades\\Typo.exe'\ndesktop = true\n",
        "\n[targets.good]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n",
    ));
    let changes = fixture.reconcile(&environment);
    assert_eq!(
        changes,
        [
            Change::Wrote {
                target: "good".to_owned(),
                icon: false,
            },
            Change::Skipped {
                target: "typo".to_owned(),
                reason: r"`C:\Hades\Typo.exe` does not exist".to_owned(),
            },
        ],
        "{changes:?}"
    );
    assert!(fixture.entry("good").is_some());
    assert_eq!(fixture.entry("typo"), None);
}

#[test]
fn check_refuses_an_exe_that_is_not_there() {
    let fixture = Fixture::new();
    assert!(check(&fixture.paths, &fixture.prefix, r"C:\Hades\Hades.exe").is_ok());
    assert!(check(&fixture.paths, &fixture.prefix, "notepad").is_ok());
    let err = check(&fixture.paths, &fixture.prefix, r"C:\Hades\Typo.exe").unwrap_err();
    assert!(matches!(err, Error::ExeNotFound(_)), "{err}");
}

// A bare name is wine's to look up, so there is nothing to check it against.
#[test]
fn accepts_a_bare_program_name() {
    let fixture = Fixture::new();
    let environment = fixture.environment("\n[targets.pad]\nexe = 'notepad'\ndesktop = true\n");
    assert_eq!(fixture.reconcile(&environment).len(), 1);
    assert!(
        fixture
            .entry("pad")
            .unwrap()
            .contains("\nStartupWMClass=notepad.exe\n")
    );
}

#[test]
fn lists_only_entries_wineo_wrote() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);

    let dir = fixture.paths.applications_dir();
    // Named like one of wineo's, but without the keys that make it wineo's.
    fs::write(
        dir.join("wineo-someone-elses.desktop"),
        "[Desktop Entry]\nName=Not ours\n",
    )
    .unwrap();
    fs::write(
        dir.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\n",
    )
    .unwrap();

    let listed = list(&fixture.paths).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].environment, "hades");
    assert_eq!(listed[0].target, "hades");
    assert_eq!(listed[0].name, "hades");
}

#[test]
fn ownership_keys_identify_a_renamed_entry() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    let renamed = fixture.paths.applications_dir().join("renamed.desktop");
    fs::rename(entry_path(&fixture.paths, "hades", "hades"), &renamed).unwrap();

    let listed = list(&fixture.paths).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, renamed);
    assert_eq!(remove_all(&fixture.paths, "hades").unwrap(), ["hades"]);
    assert!(!renamed.exists());
}

#[test]
fn remove_all_clears_one_environment() {
    let fixture = Fixture::new();
    let environment = fixture.environment(
            "\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndesktop = true\n\n[targets.pad]\nexe = 'notepad'\ndesktop = true\n",
        );
    fixture.reconcile(&environment);
    assert_eq!(list(&fixture.paths).unwrap().len(), 2);

    let removed = remove_all(&fixture.paths, "hades").unwrap();
    assert_eq!(removed, ["hades", "pad"]);
    assert!(list(&fixture.paths).unwrap().is_empty());
    // A second pass has nothing left to do.
    assert!(remove_all(&fixture.paths, "hades").unwrap().is_empty());
}

#[test]
fn entry_values_stay_on_one_line() {
    assert_eq!(clean("Hades\nInjected=x"), "HadesInjected=x");
    assert_eq!(clean("  Hades  "), "Hades");
}

#[test]
fn shortcut_icon_ignores_file_association_entries() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("wine-extension-vbs.desktop"),
        "[Desktop Entry]\nMimeType=text/vbscript;\nIcon=wscript\n",
    )
    .unwrap();
    assert_eq!(wine_icon_name(root.path()), None);

    let menu = root.path().join("wine/Programs");
    fs::create_dir_all(&menu).unwrap();
    fs::write(
        menu.join("Game.desktop"),
        "[Desktop Entry]\nName=Game\nIcon=game\n",
    )
    .unwrap();
    assert_eq!(wine_icon_name(root.path()).as_deref(), Some("game"));
}

#[test]
fn window_class_comes_from_the_executable() {
    assert_eq!(wm_class(r"C:\Hades\Hades.exe").unwrap(), "Hades.exe");
    assert_eq!(wm_class("/games/hades/Hades.exe").unwrap(), "Hades.exe");
    assert_eq!(wm_class("notepad").unwrap(), "notepad.exe");
    assert_eq!(wm_class(""), None);
}

#[test]
fn icons_go_when_the_entry_does() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);

    // Stand in for what wine's menu builder would have written.
    let icon = fixture
        .paths
        .icons_dir()
        .join("hicolor/48x48/apps")
        .join(format!("{}.png", icon_name("hades", "hades")));
    fs::create_dir_all(icon.parent().unwrap()).unwrap();
    fs::write(&icon, b"png").unwrap();
    assert!(has_icon(&fixture.paths, "hades", "hades"));

    // The entry is now out of date, since it has no Icon line yet.
    assert_eq!(
        fixture.reconcile(&environment),
        [Change::Updated {
            target: "hades".to_owned()
        }]
    );
    assert!(
        fixture
            .entry("hades")
            .unwrap()
            .contains("\nIcon=wineo-hades-hades\n")
    );

    remove_all(&fixture.paths, "hades").unwrap();
    assert!(!icon.exists());
}

#[test]
fn environment_icon_art_is_used() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    let environment = fixture.environment(ASKS);
    assert_eq!(
        fixture.reconcile(&environment),
        [Change::Wrote {
            target: "hades".to_owned(),
            icon: true,
        }]
    );
    let hash = format!("{:x}", Sha256::digest(png(64, 1)));
    let entry = fixture.entry("hades").unwrap();
    assert!(
        entry.contains(&format!(
            "\nIcon=wineo-hades-hades\nX-Wineo-Icon=art:{}\n",
            &hash[..16]
        )),
        "{entry}"
    );
    assert_eq!(fixture.icons(), ["64x64/apps/wineo-hades-hades.png"]);

    // The stamp matches, so a second pass writes nothing.
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn target_icon_art_beats_the_environments() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    fixture.art(Some("hades"), "icon.png", &png(32, 2));
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    let hash = format!("{:x}", Sha256::digest(png(32, 2)));
    assert_eq!(
        stamp_of(&fixture.entry("hades").unwrap()),
        Some(format!("art:{}", &hash[..16]).as_str())
    );
    assert_eq!(fixture.icons(), ["32x32/apps/wineo-hades-hades.png"]);
}

#[test]
fn changed_icon_art_replaces_the_icon() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    let before = stamp_of(&fixture.entry("hades").unwrap())
        .unwrap()
        .to_owned();

    fixture.art(None, "icon.png", &png(64, 2));
    assert_eq!(fixture.reconcile(&environment), updated());
    let after = fixture.entry("hades").unwrap();
    assert_ne!(stamp_of(&after).unwrap(), before);
    let icon = fixture
        .paths
        .icons_dir()
        .join("hicolor/64x64/apps/wineo-hades-hades.png");
    let written = image::open(icon).unwrap().to_rgba8();
    assert_eq!(written.get_pixel(0, 0).0, [2, 0, 0, 255]);
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn switching_formats_leaves_no_stale_icon() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.svg", SVG);
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    assert_eq!(fixture.icons(), ["scalable/apps/wineo-hades-hades.svg"]);

    fixture.clear_art(None);
    fixture.art(None, "icon.png", &png(48, 1));
    assert_eq!(fixture.reconcile(&environment), updated());
    assert_eq!(fixture.icons(), ["48x48/apps/wineo-hades-hades.png"]);

    fixture.clear_art(None);
    fixture.art(None, "icon.svg", SVG);
    assert_eq!(fixture.reconcile(&environment), updated());
    assert_eq!(fixture.icons(), ["scalable/apps/wineo-hades-hades.svg"]);
}

#[test]
fn art_replaces_every_size_of_an_exe_icon() {
    let fixture = Fixture::new();
    fixture.exe_icon("32x32");
    fixture.exe_icon("48x48");
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    assert_eq!(stamp_of(&fixture.entry("hades").unwrap()), Some("exe"));

    fixture.art(None, "icon.png", &png(256, 1));
    assert_eq!(fixture.reconcile(&environment), updated());
    assert_eq!(fixture.icons(), ["256x256/apps/wineo-hades-hades.png"]);
}

#[test]
fn removing_the_art_removes_its_icon() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);

    fixture.clear_art(None);
    assert_eq!(fixture.reconcile(&environment), updated());
    assert!(fixture.icons().is_empty());
    let entry = fixture.entry("hades").unwrap();
    assert!(!entry.contains("Icon="), "{entry}");
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn removing_a_target_override_falls_back_to_the_environments_art() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    fixture.art(Some("hades"), "icon.svg", SVG);
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    assert_eq!(fixture.icons(), ["scalable/apps/wineo-hades-hades.svg"]);

    fixture.clear_art(Some("hades"));
    assert_eq!(fixture.reconcile(&environment), updated());
    assert_eq!(fixture.icons(), ["64x64/apps/wineo-hades-hades.png"]);
}

// Entries written before the stamp existed are rewritten once, and an exe
// icon already on disk is kept without running wine.
#[test]
fn an_old_entry_with_an_exe_icon_is_stamped_once() {
    let fixture = Fixture::new();
    fixture.exe_icon("48x48");
    let old = concat!(
        "[Desktop Entry]\n",
        "Type=Application\n",
        "Name=hades\n",
        "Comment=Runs in the wineo environment hades\n",
        "Exec=wineo start hades --target hades\n",
        "Icon=wineo-hades-hades\n",
        "Terminal=false\n",
        "Categories=Utility;Software;\n",
        "StartupWMClass=Hades.exe\n",
        "X-Wineo-Environment=hades\n",
        "X-Wineo-Target=hades\n",
    );
    let path = entry_path(&fixture.paths, "hades", "hades");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, old).unwrap();
    let environment = fixture.environment(ASKS);

    assert_eq!(fixture.reconcile(&environment), updated());
    assert_eq!(
        fixture.entry("hades").unwrap(),
        old.replace(
            "Icon=wineo-hades-hades\n",
            "Icon=wineo-hades-hades\nX-Wineo-Icon=exe\n"
        )
    );
    assert_eq!(fixture.icons(), ["48x48/apps/wineo-hades-hades.png"]);
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn unreadable_art_is_reported_once() {
    let fixture = Fixture::new();
    // Passes the store's format check but does not decode.
    let mut bad = b"\x89PNG\r\n\x1a\n".to_vec();
    bad.extend_from_slice(b"not really a png");
    fixture.art(None, "icon.png", &bad);
    let environment = fixture.environment(ASKS);
    assert_eq!(
        fixture.reconcile(&environment),
        [
            Change::Wrote {
                target: "hades".to_owned(),
                icon: false,
            },
            Change::IconArtUnreadable {
                target: "hades".to_owned(),
            },
        ]
    );
    let entry = fixture.entry("hades").unwrap();
    assert!(!entry.contains("\nIcon="), "{entry}");
    assert!(stamp_of(&entry).unwrap().starts_with("art:"));
    assert!(fixture.reconcile(&environment).is_empty());

    // Replacing the file tries again.
    bad.push(b'!');
    fixture.art(None, "icon.png", &bad);
    assert_eq!(
        fixture.reconcile(&environment),
        [
            Change::Updated {
                target: "hades".to_owned(),
            },
            Change::IconArtUnreadable {
                target: "hades".to_owned(),
            },
        ]
    );
}

#[test]
fn unreadable_art_keeps_an_exe_icon() {
    let fixture = Fixture::new();
    fixture.exe_icon("48x48");
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);

    fixture.art(None, "icon.png", b"\x89PNG\r\n\x1a\nbroken");
    assert_eq!(
        fixture.reconcile(&environment),
        [
            Change::Updated {
                target: "hades".to_owned(),
            },
            Change::IconArtUnreadable {
                target: "hades".to_owned(),
            },
        ]
    );
    assert_eq!(fixture.icons(), ["48x48/apps/wineo-hades-hades.png"]);
    assert!(
        fixture
            .entry("hades")
            .unwrap()
            .contains("\nIcon=wineo-hades-hades\nX-Wineo-Icon=art:")
    );
    assert!(fixture.reconcile(&environment).is_empty());
}

#[test]
fn a_bad_art_toml_counts_as_no_art() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.png", &png(64, 1));
    fs::write(
        fixture.paths.art_dir("hades").join(art::ART_FILE),
        "schema_version = 1\ngame = \"not a number\"\n",
    )
    .unwrap();
    assert!(ArtConfig::load(&fixture.paths.art_dir("hades")).is_err());
    let environment = fixture.environment(ASKS);
    assert_eq!(
        fixture.reconcile(&environment),
        [Change::Wrote {
            target: "hades".to_owned(),
            icon: false,
        }]
    );
    assert!(!fixture.entry("hades").unwrap().contains("Icon"));
    assert!(fixture.icons().is_empty());
}

#[test]
fn remove_all_takes_an_svg_icon_too() {
    let fixture = Fixture::new();
    fixture.art(None, "icon.svg", SVG);
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    assert_eq!(fixture.icons().len(), 1);
    remove_all(&fixture.paths, "hades").unwrap();
    assert!(fixture.icons().is_empty());
}

fn pending_for(fixture: &Fixture, environment: &Environment) -> Vec<Pending> {
    pending(
        &fixture.paths,
        &GlobalConfig::default(),
        environment,
        None,
        &fixture.prefix,
    )
    .unwrap()
}

#[test]
fn pending_says_what_reconcile_would_do_without_writing() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    let hades = || "hades".to_owned();
    assert_eq!(
        pending_for(&fixture, &environment),
        [Pending::Write { target: hades() }]
    );
    assert!(fixture.entry("hades").is_none());

    fixture.reconcile(&environment);
    assert!(pending_for(&fixture, &environment).is_empty());

    let renamed = fixture.environment(&format!("{ASKS}name = 'Hades II'\n"));
    let before = fixture.entry("hades");
    assert_eq!(
        pending_for(&fixture, &renamed),
        [Pending::Update { target: hades() }]
    );
    assert_eq!(fixture.entry("hades"), before);

    let dropped = fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\n");
    assert_eq!(
        pending_for(&fixture, &dropped),
        [Pending::Remove { target: hades() }]
    );
    assert_eq!(fixture.entry("hades"), before);
}

#[test]
fn pending_sees_changed_icon_art_without_installing_it() {
    let fixture = Fixture::new();
    let environment = fixture.environment(ASKS);
    fixture.reconcile(&environment);
    fixture.art(None, "icon.png", &png(64, 1));
    assert_eq!(
        pending_for(&fixture, &environment),
        [Pending::Update {
            target: "hades".to_owned()
        }]
    );
    assert!(fixture.icons().is_empty());
}

#[test]
fn pending_leaves_out_a_target_that_would_be_skipped() {
    let fixture = Fixture::new();
    let environment =
        fixture.environment("\n[targets.hades]\nexe = 'C:\\Hades\\Missing.exe'\ndesktop = true\n");
    assert!(pending_for(&fixture, &environment).is_empty());
}

#[test]
fn proxied_entries_use_the_implicit_target_and_title_fallbacks() {
    let fixture = Fixture::new();
    let manifest = fixture.steam_game("Live title");
    let environment = fixture.proxied("title = 'Stored title'\ndesktop = true\n");
    assert_eq!(
        fixture.reconcile_proxied(&environment),
        [Change::Wrote {
            target: "steam".to_owned(),
            icon: false,
        }]
    );
    let entry = fixture.entry("steam").unwrap();
    assert!(entry.contains("Name=Live title\n"), "{entry}");
    assert!(entry.contains("Exec=wineo start hades\n"), "{entry}");
    assert!(entry.contains("Categories=Game;\n"), "{entry}");
    assert!(entry.contains("X-Wineo-Target=steam\n"), "{entry}");
    assert!(!entry.lines().any(|line| line.starts_with("Icon=")));

    fs::remove_file(manifest).unwrap();
    let environment = fixture.proxied("title = 'Stored title'\ndesktop = true\n");
    assert_eq!(
        fixture.reconcile_proxied(&environment),
        [Change::Updated {
            target: "steam".to_owned()
        }]
    );
    assert!(
        fixture
            .entry("steam")
            .unwrap()
            .contains("Name=Stored title\n")
    );

    let environment = fixture.proxied("desktop = true\n");
    fixture.reconcile_proxied(&environment);
    assert!(fixture.entry("steam").unwrap().contains("Name=hades\n"));
}

#[test]
fn proxied_entry_does_not_replace_or_remove_a_native_name_collision() {
    let fixture = Fixture::new();
    let native_config = fixture.paths.environment_config("x");
    fs::create_dir_all(native_config.parent().unwrap()).unwrap();
    fs::write(
        &native_config,
        "kind = 'native'\n[targets.foo-steam]\nexe = '/bin/true'\ndesktop = true\n",
    )
    .unwrap();
    let native = Environment::load(&fixture.paths, "x").unwrap();
    assert_eq!(
        reconcile_native(&fixture.paths, &GlobalConfig::default(), &native).unwrap(),
        [Change::Wrote {
            target: "foo-steam".to_owned(),
            icon: false,
        }]
    );
    let path = entry_path(&fixture.paths, "x", "foo-steam");
    let native_entry = fs::read(&path).unwrap();

    let proxied_config = fixture.paths.environment_config("x-foo");
    fs::write(
        &proxied_config,
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '77'\ndesktop = true\n",
    )
    .unwrap();
    let proxied = Environment::load(&fixture.paths, "x-foo").unwrap();
    assert_eq!(
        reconcile_proxied(&fixture.paths, &GlobalConfig::default(), &proxied).unwrap(),
        [Change::Skipped {
            target: "steam".to_owned(),
            reason: Error::DesktopEntryConflict(path.clone()).to_string(),
        }]
    );
    assert_eq!(fs::read(&path).unwrap(), native_entry);

    fs::write(
        &proxied_config,
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '77'\ndesktop = false\n",
    )
    .unwrap();
    let disabled = Environment::load(&fixture.paths, "x-foo").unwrap();
    assert!(
        reconcile_proxied(&fixture.paths, &GlobalConfig::default(), &disabled)
            .unwrap()
            .is_empty()
    );
    assert!(remove_all(&fixture.paths, "x-foo").unwrap().is_empty());
    assert_eq!(fs::read(&path).unwrap(), native_entry);
}

#[test]
fn proxied_entry_does_not_replace_an_unowned_file() {
    let fixture = Fixture::new();
    let environment = fixture.proxied("desktop = true\n");
    let path = entry_path(&fixture.paths, "hades", "steam");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "unowned shortcut\n").unwrap();

    assert_eq!(
        fixture.reconcile_proxied(&environment),
        [Change::Skipped {
            target: "steam".to_owned(),
            reason: Error::DesktopEntryConflict(path.clone()).to_string(),
        }]
    );
    assert_eq!(fs::read(&path).unwrap(), b"unowned shortcut\n");
}

#[test]
fn proxied_reconcile_drops_a_leftover_exe_icon() {
    let fixture = Fixture::new();
    let environment = fixture.proxied("desktop = true\n");
    fixture.reconcile_proxied(&environment);
    let path = entry_path(&fixture.paths, "hades", "steam");
    let mut entry = fs::read_to_string(&path).unwrap();
    entry = entry.replace(
        "Terminal=false\n",
        "Icon=wineo-hades-steam\nX-Wineo-Icon=exe\nTerminal=false\n",
    );
    fs::write(&path, entry).unwrap();
    let icon = fixture
        .paths
        .icons_dir()
        .join("hicolor/48x48/apps/wineo-hades-steam.png");
    fs::create_dir_all(icon.parent().unwrap()).unwrap();
    fs::write(&icon, b"old exe icon").unwrap();

    assert_eq!(
        fixture.reconcile_proxied(&environment),
        [Change::Updated {
            target: "steam".to_owned()
        }]
    );
    let entry = fs::read_to_string(path).unwrap();
    assert!(!entry.lines().any(|line| line.starts_with("Icon=")));
    assert!(!entry.lines().any(|line| line == "X-Wineo-Icon=exe"));
    assert!(!icon.exists());
}

#[test]
fn proxied_entry_uses_wineo_art_and_removes_entry_and_icon() {
    let fixture = Fixture::new();
    fixture.art(Some("steam"), "icon.png", &png(64, 1));
    let environment = fixture.proxied("desktop = true\n");
    assert_eq!(
        fixture.reconcile_proxied(&environment),
        [Change::Wrote {
            target: "steam".to_owned(),
            icon: true,
        }]
    );
    assert!(
        fixture
            .entry("steam")
            .unwrap()
            .contains("Icon=wineo-hades-steam\n")
    );
    let icon = fixture
        .paths
        .icons_dir()
        .join("hicolor/64x64/apps/wineo-hades-steam.png");
    assert!(icon.is_file());

    assert_eq!(remove_all(&fixture.paths, "hades").unwrap(), ["steam"]);
    assert!(fixture.entry("steam").is_none());
    assert!(!icon.exists());
}

#[test]
fn proxied_pending_reports_write_and_removal() {
    let fixture = Fixture::new();
    let environment = fixture.proxied("desktop = true\n");
    assert_eq!(
        pending_proxied(&fixture.paths, &GlobalConfig::default(), &environment).unwrap(),
        [Pending::Write {
            target: "steam".to_owned()
        }]
    );
    fixture.reconcile_proxied(&environment);
    let disabled = fixture.proxied("");
    assert_eq!(
        pending_proxied(&fixture.paths, &GlobalConfig::default(), &disabled).unwrap(),
        [Pending::Remove {
            target: "steam".to_owned()
        }]
    );
    fixture.reconcile_proxied(&disabled);
    assert!(fixture.entry("steam").is_none());
}

#[test]
fn proxied_entries_are_skipped_in_portable_mode() {
    let fixture = Fixture::new();
    let home = fixture._root.path().to_owned();
    let portable_root = home.join("portable");
    let paths = Paths::select(
        move |key| {
            let path = match key {
                "HOME" => home.clone(),
                "XDG_CONFIG_HOME" => home.join("config"),
                "XDG_DATA_HOME" => home.join("data"),
                "XDG_STATE_HOME" => home.join("state"),
                _ => return None,
            };
            Some(OsString::from(path.as_os_str()))
        },
        Some(&portable_root),
    )
    .unwrap();
    let config = paths.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\ndesktop = true\n",
    )
    .unwrap();
    let environment = Environment::load(&paths, "hades").unwrap();

    assert_eq!(
        reconcile_proxied(&paths, &GlobalConfig::default(), &environment).unwrap(),
        [Change::Skipped {
            target: "steam".to_owned(),
            reason: Error::PortableDesktop.to_string(),
        }]
    );
    assert!(!entry_path(&paths, "hades", "steam").exists());
}
