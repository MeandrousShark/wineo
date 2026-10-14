use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;

use wineo_core::config::EnvironmentConfig;

use super::*;
use crate::prompt::Script;

struct Fixture {
    root: tempfile::TempDir,
    paths: Paths,
    wine_prefix: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let lookup = home.clone();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(lookup.as_os_str()))
        })
        .unwrap();
        let wine_prefix = home.join("prefix");
        fs::create_dir_all(wine_prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", wine_prefix.join("dosdevices/c:")).unwrap();
        for exe in [
            "drive_c/Games/Hades/Hades.exe",
            "drive_c/Games/Hades/Config.exe",
            "drive_c/Games/Hades/unins000.exe",
        ] {
            let path = wine_prefix.join(exe);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
        Self {
            root,
            paths,
            wine_prefix,
        }
    }

    fn place(&self) -> Place<'_> {
        Place {
            paths: &self.paths,
            name: "hades",
            wine_prefix: Some(self.wine_prefix.clone()),
            native_env: None,
            game_folder: None,
        }
    }
}

fn batch(targets: &str) -> Batch {
    let text = format!("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n{targets}");
    Batch::new(
        EnvironmentConfig::parse(Path::new("/x.toml"), &text)
            .unwrap()
            .targets,
    )
}

fn run<T>(
    answers: &[&str],
    flow: impl FnOnce(&mut Prompt<Script, Vec<u8>>) -> anyhow::Result<T>,
) -> (anyhow::Result<T>, String) {
    let mut prompt = Prompt::new(Script::new(answers), Vec::new());
    let result = flow(&mut prompt);
    let printed = String::from_utf8_lossy(prompt.output()).into_owned();
    (result, printed)
}

#[test]
fn picking_programs_adds_them_with_suggested_settings() {
    let f = Fixture::new();
    let mut batch = batch("");
    let (result, printed) = run(&["1 2", "", ""], |prompt| {
        pick(prompt, &f.place(), &mut batch)
    });
    result.unwrap();
    assert_eq!(
        printed.matches("Programs in hades:").count(),
        1,
        "{printed}"
    );
    // Noise is held back, and named in the menu rather than listed.
    assert!(
        printed.contains("a) show 1 held back as noise"),
        "{printed}"
    );
    assert!(!printed.contains("unins000"), "{printed}");
    let targets = batch.targets();
    assert_eq!(targets.keys().collect::<Vec<_>>(), ["config", "hades"]);
    assert_eq!(targets["hades"].exe, r"C:\Games\Hades\Hades.exe");
    assert!(targets["hades"].desktop);
    assert_eq!(targets["hades"].name.as_deref(), Some("Hades"));
    assert!(targets["hades"].args.is_empty());
    assert_eq!(targets["hades"].cwd, None);
}

#[test]
fn backing_out_of_a_card_shows_the_programs_again() {
    let f = Fixture::new();
    let mut batch = batch("");
    let (result, printed) = run(&["1", "b", "s"], |prompt| {
        pick(prompt, &f.place(), &mut batch)
    });
    assert_eq!(result.unwrap(), Reply::Value(()));
    assert!(batch.is_empty());
    assert_eq!(
        printed.matches("Programs in hades:").count(),
        2,
        "{printed}"
    );
}

#[test]
fn a_card_changes_one_field_at_a_time() {
    let f = Fixture::new();
    let mut batch = batch("");
    let answers = [
        "1",
        // Rename it, then give it two arguments, one with spaces.
        "1",
        "game",
        "2",
        "-config",
        r"C:\My Games\settings.ini",
        "",
        // Working directory as a Unix path inside drive_c.
        "3",
        &format!("{}/drive_c/Games", f.wine_prefix.display()),
        // No desktop entry after all.
        "4",
        "n",
        "",
    ];
    let (result, printed) = run(&answers, |prompt| pick(prompt, &f.place(), &mut batch));
    result.unwrap();
    let targets = batch.targets();
    let game = &targets["game"];
    assert_eq!(game.args, ["-config", r"C:\My Games\settings.ini"]);
    assert_eq!(game.cwd.as_deref(), Some(r"C:\Games"));
    assert!(!game.desktop);
    assert_eq!(game.name, None);
    assert!(
        printed.contains(r#"  arguments    -config "C:\My Games\settings.ini""#),
        "{printed}"
    );
}

#[test]
fn an_executable_in_use_is_not_added_again_without_asking() {
    let f = Fixture::new();
    let mut batch = batch("[targets.hades]\nexe = 'C:\\Games\\Hades\\Hades.exe'\n");
    // Say no to the duplicate, then yes, which suggests a free name.
    let (result, printed) = run(&["2", "", "2", "y", ""], |prompt| {
        pick(prompt, &f.place(), &mut batch)
    });
    result.unwrap();
    assert_eq!(
        printed.matches("Programs in hades:").count(),
        2,
        "{printed}"
    );
    assert!(printed.contains("(target hades)"), "{printed}");
    assert!(
        printed.contains(r"C:\Games\Hades\Hades.exe is already target hades"),
        "{printed}"
    );
    assert!(
        printed.contains("`hades` is taken, so this one is `hades-2`."),
        "{printed}"
    );
    assert_eq!(
        batch.targets().keys().collect::<Vec<_>>(),
        ["hades", "hades-2"]
    );
}

#[test]
fn a_path_can_be_entered_with_spaces_and_outside_the_prefix() {
    let f = Fixture::new();
    let outside = f
        .root
        .path()
        .canonicalize()
        .unwrap()
        .join("My Apps/tool.exe");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, b"").unwrap();
    let mut batch = batch("");
    let answers = ["p", "/nowhere/nothing.exe", outside.to_str().unwrap(), ""];
    let (result, printed) = run(&answers, |prompt| pick(prompt, &f.place(), &mut batch));
    result.unwrap();
    assert!(
        printed.contains("`/nowhere/nothing.exe` does not exist"),
        "{printed}"
    );
    // Stored the way `wineo new` stores paths under the home directory.
    assert_eq!(batch.targets()["tool"].exe, "~/My Apps/tool.exe");
}

#[test]
fn an_empty_prefix_still_takes_a_path() {
    let f = Fixture::new();
    let place = Place {
        wine_prefix: None,
        ..f.place()
    };
    let mut batch = batch("");
    let (result, printed) = run(&["s"], |prompt| pick(prompt, &place, &mut batch));
    result.unwrap();
    assert!(printed.contains("Nothing installed was found in hades."));
    assert!(printed.contains("  p) enter a path\n"), "{printed}");
}

#[test]
fn removing_the_default_asks_for_another() {
    let f = Fixture::new();
    let mut batch = batch(
        "[targets.a]\nexe = 'a'\ndefault = true\ndesktop = true\n[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'c'\n",
    );
    let (result, printed) = run(&["1", "1", "2"], |prompt| {
        remove(prompt, &f.place(), &mut batch)
    });
    result.unwrap();
    assert!(printed.contains("its entry \"a\" goes too"), "{printed}");
    assert!(
        printed.contains("The program's files and the prefix are left alone."),
        "{printed}"
    );
    assert_eq!(batch.default().as_deref(), Some("c"));
    assert_eq!(
        review(&batch),
        ["  - a  a, and its desktop entry", "  default: c (was a)"]
    );
}

#[test]
fn editing_keeps_the_desktop_choice_and_can_move_the_default() {
    let f = Fixture::new();
    let mut batch = batch(
        "[targets.a]\nexe = 'a'\ndefault = true\n[targets.b]\nexe = 'b'\ndesktop = true\nname = \"Bee\"\n",
    );
    let (result, printed) = run(&["2", "2", "-x", "", "5", ""], |prompt| {
        edit(prompt, &f.place(), &mut batch)
    });
    result.unwrap();
    assert!(
        printed.contains("  desktop      yes, named \"Bee\""),
        "{printed}"
    );
    let targets: BTreeMap<_, _> = batch.targets();
    assert!(targets["b"].desktop);
    assert_eq!(targets["b"].args, ["-x"]);
    assert_eq!(batch.default().as_deref(), Some("b"));
    assert_eq!(
        review(&batch),
        ["  ~ b  arguments: none -> -x", "  default: b (was a)"]
    );
}

#[test]
fn editing_a_lone_target_picks_it_by_default() {
    let f = Fixture::new();
    let mut batch = batch("[targets.a]\nexe = 'a'\n");
    let (result, _) = run(&["", "2", "-x", "", ""], |prompt| {
        edit(prompt, &f.place(), &mut batch)
    });
    result.unwrap();
    assert_eq!(batch.targets()["a"].args, ["-x"]);
}

#[test]
fn back_leaves_the_batch_as_it_was() {
    let f = Fixture::new();
    let mut batch = batch("[targets.a]\nexe = 'a'\n");
    let (result, _) = run(&["1", "2", "-x", "", "b"], |prompt| {
        edit(prompt, &f.place(), &mut batch)
    });
    assert_eq!(result.unwrap(), Reply::Back);
    assert!(batch.is_empty());
}

#[test]
fn a_bad_name_says_what_is_wrong() {
    assert_eq!(name_problem("Stellar-Blade"), None);
    assert_eq!(
        name_problem("Stellar Blade").unwrap(),
        "Invalid input: spaces not allowed."
    );
    assert_eq!(
        name_problem("Hades: II").unwrap(),
        "Invalid input: `:` and spaces not allowed."
    );
}

#[test]
fn a_pasted_path_loses_its_quotes_and_escapes() {
    assert_eq!(
        clean_path(r#" "~/Games/Stellar Blade/SB.exe" "#),
        "~/Games/Stellar Blade/SB.exe"
    );
    assert_eq!(clean_path("'/games/My Game/g.exe'"), "/games/My Game/g.exe");
    assert_eq!(
        clean_path(r"~/Games/Stellar\ Blade/SB.exe"),
        "~/Games/Stellar Blade/SB.exe"
    );
    // A Windows path's backslashes are its separators.
    assert_eq!(
        clean_path(r#""C:\Games\ SB\SB.exe""#),
        r"C:\Games\ SB\SB.exe"
    );
    assert_eq!(clean_path(r#""unbalanced"#), r#""unbalanced"#);
}

#[test]
fn stems_come_from_either_kind_of_path() {
    assert_eq!(file_stem(r"C:\Games\Hades\Hades.exe"), "Hades");
    assert_eq!(file_stem("~/My Apps/tool.exe"), "tool");
    assert_eq!(file_stem("run"), "run");
}
