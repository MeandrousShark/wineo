use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::*;

fn parse(text: &str) -> BTreeMap<String, Target> {
    let text = format!("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n{text}");
    EnvironmentConfig::parse(Path::new("/x.toml"), &text)
        .unwrap()
        .targets
}

fn spec(exe: &str) -> Spec {
    Spec {
        exe: exe.to_owned(),
        ..Spec::default()
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let path = paths.environment_config("hades");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, config).unwrap();
        Self { _root: root, paths }
    }

    fn text(&self) -> String {
        fs::read_to_string(self.paths.environment_config("hades")).unwrap()
    }

    fn targets(&self) -> BTreeMap<String, Target> {
        EnvironmentConfig::parse(Path::new("/x.toml"), &self.text())
            .unwrap()
            .targets
    }
}

const HEADER: &str = "# my notes\nprefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\ngamemode = true\n\n[env]\nDXVK_HUD = \"fps\"\n";

#[test]
fn a_portable_target_can_not_gain_a_desktop_entry() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&root.path().join("usb")),
    )
    .unwrap();
    let path = paths.environment_config("hades");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "prefix = 'prefixes/hades'\nrunner = 'wine'\narch = 'win64'\n[targets.old]\nexe = 'old'\ndesktop = true\n";
    fs::write(&path, text).unwrap();
    let base = EnvironmentConfig::parse(&path, text).unwrap().targets;

    let mut batch = Batch::new(base.clone());
    let mut wanted = spec("new");
    wanted.desktop = true;
    batch.add("new", wanted).unwrap();
    assert!(matches!(
        write(&paths, "hades", &batch),
        Err(Error::PortableDesktop)
    ));

    // A hand-edited one is left as it is when the target changes otherwise.
    let mut batch = Batch::new(base.clone());
    let mut edited = Spec::of(&base["old"]);
    edited.exe = "older".to_owned();
    batch.edit("old", edited);
    assert!(matches!(write(&paths, "hades", &batch), Ok(Written::Saved)));
}

#[test]
fn names_come_from_labels_then_executables() {
    assert_eq!(suggest_name("Hades II", r"C:\x\Hades2.exe"), "hades-ii");
    assert_eq!(
        suggest_name("Hades: Battle Out of Hell", ""),
        "hades-battle-out-of-hell"
    );
    assert_eq!(suggest_name("My_Game v1.2", ""), "my_game-v1-2");
    assert_eq!(suggest_name("  --Fly Corp--  ", ""), "fly-corp");
    // Nothing usable in the label, so the executable names it.
    assert_eq!(suggest_name("游戏", r"C:\Games\Game.exe"), "game");
    assert_eq!(suggest_name("", "/opt/tools/run.sh"), "run");
    assert_eq!(suggest_name("™", "™.exe"), "app");
    for label in ["Hades II", "My_Game v1.2", "  --Fly Corp--  "] {
        assert!(name::is_valid(&suggest_name(label, "")), "{label}");
    }
}

#[test]
fn a_taken_name_gets_the_first_free_suffix() {
    let taken = ["hades", "hades-2"];
    assert_eq!(free_name("hades", |name| taken.contains(&name)), "hades-3");
    assert_eq!(free_name("config", |name| taken.contains(&name)), "config");
}

#[test]
fn a_batch_shows_the_targets_it_will_leave() {
    let mut batch = Batch::new(parse(
        "[targets.hades]\nexe = 'C:\\Hades\\Hades.exe'\ndefault = true\n[targets.old]\nexe = 'old.exe'\n",
    ));
    batch.add("config", spec(r"C:\Hades\Config.exe")).unwrap();
    batch.remove("old");
    let names: Vec<String> = batch.targets().into_keys().collect();
    assert_eq!(names, ["config", "hades"]);
    assert_eq!(batch.default().as_deref(), Some("hades"));
    assert!(!batch.contains("old"));

    assert_eq!(
        batch.add("hades", spec("x")),
        Err(Refused::Taken("hades".to_owned()))
    );
    assert_eq!(
        batch.add("bad name", spec("x")),
        Err(Refused::InvalidName("bad name".to_owned()))
    );
    // A removed name is free again.
    assert!(batch.add("old", spec("new.exe")).is_ok());
}

#[test]
fn undoing_a_change_leaves_nothing_to_write() {
    let base = parse("[targets.hades]\nexe = 'Hades.exe'\n");
    let mut batch = Batch::new(base.clone());
    let mut edited = Spec::of(&base["hades"]);
    edited.args = vec!["-windowed".to_owned()];
    batch.edit("hades", edited);
    assert!(!batch.is_empty());
    batch.edit("hades", Spec::of(&base["hades"]));
    assert!(batch.is_empty());

    batch.add("tmp", spec("tmp.exe")).unwrap();
    batch.remove("tmp");
    assert!(batch.is_empty());
}

#[test]
fn removing_the_default_leaves_none_until_one_is_chosen() {
    let mut batch = Batch::new(parse(
        "[targets.a]\nexe = 'a'\ndefault = true\n[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'c'\n",
    ));
    batch.remove("a");
    assert_eq!(batch.default(), None);
    batch.set_default(Some("c"));
    assert_eq!(
        batch.summary(),
        [
            Change::Removed {
                target: "a".to_owned(),
                spec: spec("a"),
            },
            Change::Default {
                before: Some("a".to_owned()),
                after: Some("c".to_owned()),
            },
        ]
    );
}

#[test]
fn writing_refuses_an_unsupported_version() {
    let text = format!("schema_version = 2\n{HEADER}");
    let f = Fixture::new(&text);
    let mut batch = Batch::new(BTreeMap::new());
    batch.add("game", spec("game.exe")).unwrap();
    assert!(matches!(
        write(&f.paths, "hades", &batch),
        Err(Error::SchemaVersion { .. })
    ));
    assert_eq!(f.text(), text);
}

#[test]
fn writing_keeps_everything_the_batch_does_not_touch() {
    let f = Fixture::new(&format!(
        "{HEADER}\n[targets.hades]\n# the real game\nexe = 'C:\\Hades\\Hades.exe' # keep me\ngamemode = false\ndefault = true\n\n[targets.old]\nexe = 'old.exe'\n"
    ));
    let mut batch = Batch::new(f.targets());
    let mut hades = Spec::of(&f.targets()["hades"]);
    hades.args = vec!["-config".to_owned(), r"C:\My Games\settings.ini".to_owned()];
    hades.desktop = true;
    hades.name = Some("Hades".to_owned());
    batch.edit("hades", hades);
    batch
        .add(
            "config",
            Spec {
                cwd: Some(r"C:\Hades".to_owned()),
                ..spec(r"C:\Hades\Config.exe")
            },
        )
        .unwrap();
    batch.remove("old");
    assert!(matches!(
        write(&f.paths, "hades", &batch).unwrap(),
        Written::Saved
    ));

    let text = f.text();
    let stamped = HEADER.replace("gamemode = true\n", "gamemode = true\nschema_version = 1\n");
    assert!(text.starts_with(&stamped), "{text}");
    assert!(text.contains("# the real game\n"), "{text}");
    assert!(
        text.contains("exe = 'C:\\Hades\\Hades.exe' # keep me\n"),
        "{text}"
    );
    assert!(text.contains("gamemode = false\n"), "{text}");
    assert!(!text.contains("old.exe"), "{text}");
    let targets = f.targets();
    // Each argument stays one value, spaces and all.
    assert_eq!(
        targets["hades"].args,
        ["-config", r"C:\My Games\settings.ini"]
    );
    assert!(targets["hades"].desktop);
    assert!(targets["hades"].default);
    assert_eq!(targets["hades"].gamemode, Some(false));
    assert_eq!(targets["config"].cwd.as_deref(), Some(r"C:\Hades"));
    assert!(!targets["config"].desktop);
}

#[test]
fn moving_the_default_is_one_write() {
    let f = Fixture::new(&format!(
        "{HEADER}\n[targets.a]\nexe = 'a'\ndefault = true\n\n[targets.b]\nexe = 'b'\n"
    ));
    let mut batch = Batch::new(f.targets());
    batch.set_default(Some("b"));
    write(&f.paths, "hades", &batch).unwrap();
    let targets = f.targets();
    assert!(!targets["a"].default);
    assert!(targets["b"].default);
}

#[test]
fn a_change_made_meanwhile_is_not_overwritten() {
    let f = Fixture::new(&format!(
        "{HEADER}\n[targets.hades]\nexe = 'Hades.exe'\n\n[targets.other]\nexe = 'other.exe'\n"
    ));
    let mut batch = Batch::new(f.targets());
    batch.edit(
        "hades",
        Spec {
            args: vec!["-windowed".to_owned()],
            ..Spec::of(&f.targets()["hades"])
        },
    );

    // Something the batch doesn't touch is fine to change meanwhile.
    crate::settings::update(
        &f.paths,
        crate::settings::ConfigFile::Environment("hades"),
        &[("targets.other.args", r#"["-x"]"#)],
        &[],
    )
    .unwrap();
    assert!(matches!(
        write(&f.paths, "hades", &batch).unwrap(),
        Written::Saved
    ));
    assert_eq!(f.targets()["other"].args, ["-x"]);
    assert_eq!(f.targets()["hades"].args, ["-windowed"]);

    // Something it does touch refuses the write, and rebasing carries the
    // change over for a second review, keeping what changed meanwhile.
    let mut batch = Batch::new(f.targets());
    batch.edit(
        "hades",
        Spec {
            cwd: Some("C:\\".to_owned()),
            ..Spec::of(&f.targets()["hades"])
        },
    );
    crate::settings::update(
        &f.paths,
        crate::settings::ConfigFile::Environment("hades"),
        &[("targets.hades.args", r#"["-safe"]"#)],
        &[],
    )
    .unwrap();
    let before = f.text();
    let Written::Changed(current) = write(&f.paths, "hades", &batch).unwrap() else {
        panic!("a stale batch was written");
    };
    assert_eq!(f.text(), before);
    let (rebased, dropped) = batch.rebase(current);
    assert!(dropped.is_empty(), "{dropped:?}");
    assert!(matches!(
        rebased.summary().as_slice(),
        [Change::Edited { before, after, .. }]
            if before.args == ["-safe"] && after.args == ["-safe"] && after.cwd.as_deref() == Some("C:\\")
    ));
}

#[test]
fn a_default_chosen_meanwhile_refuses_a_different_one() {
    let f = Fixture::new(&format!(
        "{HEADER}\n[targets.a]\nexe = 'a'\n\n[targets.b]\nexe = 'b'\n"
    ));
    let mut batch = Batch::new(f.targets());
    batch.set_default(Some("a"));
    crate::settings::update(
        &f.paths,
        crate::settings::ConfigFile::Environment("hades"),
        &[("targets.b.default", "true")],
        &[],
    )
    .unwrap();
    assert!(matches!(
        write(&f.paths, "hades", &batch).unwrap(),
        Written::Changed(_)
    ));
}

#[test]
fn rebasing_drops_what_no_longer_applies() {
    let base = parse("[targets.a]\nexe = 'a'\n[targets.b]\nexe = 'b'\n");
    let mut batch = Batch::new(base);
    batch.edit(
        "a",
        Spec {
            args: vec!["-x".to_owned()],
            ..spec("a")
        },
    );
    batch.add("c", spec("c")).unwrap();
    batch.set_default(Some("a"));
    // Meanwhile `a` was removed and a `c` was added.
    let current = parse("[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'other'\n");
    let (rebased, dropped) = batch.rebase(current);
    assert_eq!(dropped.len(), 3, "{dropped:?}");
    assert!(rebased.is_empty());
}

#[test]
fn executables_already_in_use_are_found_whatever_the_case() {
    let batch = Batch::new(parse(
        "[targets.hades]\nexe = 'C:\\Games\\Hades\\Hades.exe'\n[targets.tool]\nexe = '/opt/Tool.exe'\n",
    ));
    assert_eq!(batch.using(r"c:\games\hades\HADES.EXE"), ["hades"]);
    assert_eq!(batch.using("C:/Games/Hades/Hades.exe"), ["hades"]);
    assert_eq!(batch.using("/opt/Tool.exe"), ["tool"]);
    assert!(batch.using("/opt/tool.exe").is_empty());
}

/// Art for `target`, set up by hand the way `art set` leaves it.
fn target_art(paths: &Paths, target: &str) -> std::path::PathBuf {
    let art = paths.art_dir("hades");
    fs::create_dir_all(art.join(target)).unwrap();
    fs::write(art.join(target).join("icon.png"), b"x").unwrap();
    fs::write(
        art.join("art.toml"),
        format!(
            "schema_version = 1\n[targets.{target}.icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n"
        ),
    )
    .unwrap();
    art
}

#[test]
fn a_removed_target_takes_its_art_along() {
    let f = Fixture::new(&format!(
        "{HEADER}\n[targets.old]\nexe = \"old\"\n\n[targets.kept]\nexe = \"kept\"\n"
    ));
    let art = target_art(&f.paths, "old");
    fs::create_dir_all(art.join("kept")).unwrap();

    let mut batch = Batch::new(f.targets());
    batch.edit("kept", spec("kept2"));
    assert!(matches!(
        write(&f.paths, "hades", &batch),
        Ok(Written::Saved)
    ));
    assert!(art.join("old/icon.png").exists());

    let mut batch = Batch::new(f.targets());
    batch.remove("old");
    assert!(matches!(
        write(&f.paths, "hades", &batch),
        Ok(Written::Saved)
    ));
    assert!(!art.join("old").exists());
    assert!(art.join("kept").exists());
    assert_eq!(
        fs::read_to_string(art.join("art.toml")).unwrap(),
        "schema_version = 1\n"
    );
}

#[test]
fn an_invalid_art_toml_stops_a_batch_that_removes_a_target() {
    let f = Fixture::new(&format!("{HEADER}\n[targets.old]\nexe = \"old\"\n"));
    let art = target_art(&f.paths, "old");
    fs::write(art.join("art.toml"), "schema_version = 2\n").unwrap();
    let before = f.text();
    let mut batch = Batch::new(f.targets());
    batch.remove("old");
    let err = write(&f.paths, "hades", &batch).unwrap_err();
    assert!(matches!(err, Error::SchemaVersion { .. }), "{err}");
    assert_eq!(f.text(), before);
    assert!(art.join("old/icon.png").exists());
}
