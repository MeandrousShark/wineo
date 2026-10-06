//! `wineo runners` with builds from Steam, Heroic, and Lutris, and refusing
//! to remove what isn't wineo's. The one-time question is in `guided.rs`.

mod common;

use std::fs;

use common::*;

/// The words of the row that starts with `name`.
fn row(text: &str, name: &str) -> Vec<String> {
    text.lines()
        .find(|line| line.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no row for {name} in:\n{text}"))
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn on() -> &'static str {
    "external_runners = true\n"
}

#[test]
fn the_source_column_names_wineo_and_each_launcher() {
    let f = Fixture::new();
    f.global_config(on());
    f.install_proton("GE-Proton9-20");
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");
    f.install_external_proton("heroic-proton", "GE-Proton11-1", "GE-Proton11-1");
    f.install_external_wine("lutris", "lutris-9");

    let listed = f.run_at_home(&["runners"]);
    assert_success(&listed);
    let text = stdout(&listed);
    assert_eq!(
        row(&text, "GE-Proton9-20")[..3],
        ["GE-Proton9-20", "proton", "wineo"]
    );
    assert_eq!(
        row(&text, "GE-Proton11-3")[..3],
        ["GE-Proton11-3", "proton", "Steam"]
    );
    assert_eq!(
        row(&text, "GE-Proton11-1")[..3],
        ["GE-Proton11-1", "proton", "Heroic"]
    );
    assert_eq!(row(&text, "lutris-9")[..3], ["lutris-9", "wine", "Lutris"]);

    // The alias rows keep their shape, and the source column lines up.
    assert!(
        row(&text, "GE-Proton-Latest")[1..].starts_with(&["alias".to_owned()]),
        "{text}"
    );
    let at = |name: &str, word: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .unwrap()
            .find(word)
    };
    assert_eq!(
        at("GE-Proton11-3", "Steam"),
        at("GE-Proton9-20", "wineo"),
        "{text}"
    );
}

#[test]
fn nothing_external_is_listed_until_scanning_is_on() {
    let f = Fixture::new();
    f.install_proton("GE-Proton9-20");
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");

    for setting in [None, Some("external_runners = false\n")] {
        if let Some(setting) = setting {
            f.global_config(setting);
        }
        let text = stdout(&f.run_at_home(&["runners", "--all"]));
        assert!(!text.contains("GE-Proton11-3"), "{text}");
        assert_eq!(row(&text, "GE-Proton9-20")[2], "wineo", "{text}");
    }
}

#[test]
fn a_hidden_build_that_differs_gets_a_note_and_a_same_one_stays_quiet() {
    let f = Fixture::new();
    f.global_config(on());
    f.install_proton("GE-Proton9-20");
    f.install_proton("GE-Proton9-21");
    // Same names as wineo's, another build in Heroic and the same one in Steam.
    f.install_external_proton("heroic-proton", "GE-Proton9-20", "GE-Proton10-3");
    f.install_external_proton("steam", "GE-Proton9-21", "GE-Proton9-21");

    let listed = f.run_at_home(&["runners"]);
    assert_success(&listed);
    let text = stdout(&listed);
    let different = text
        .lines()
        .find(|line| line.starts_with("GE-Proton9-20"))
        .unwrap();
    assert!(
        different.ends_with("(Heroic has a different build)"),
        "{text}"
    );
    let same = text
        .lines()
        .find(|line| line.starts_with("GE-Proton9-21"))
        .unwrap();
    assert!(!same.contains("different"), "{text}");
    assert_eq!(row(&text, "GE-Proton9-20")[2], "wineo", "{text}");
}

#[test]
fn all_lists_unusable_builds_from_other_launchers_with_their_source() {
    let f = Fixture::new();
    f.global_config(on());
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");
    let broken = f.external_dir("steam").join("Broken-Proton");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("proton"), "not executable").unwrap();

    let plain = stdout(&f.run_at_home(&["runners"]));
    assert!(!plain.contains("Broken-Proton"), "{plain}");

    let all = f.run_at_home(&["runners", "--all"]);
    assert_success(&all);
    let text = stdout(&all);
    let broken = row(&text, "Broken-Proton");
    assert_eq!(broken[1], "Steam", "{text}");
    assert_eq!(broken[2], "skipped,", "{text}");
    assert!(
        row(&text, "GE-Proton11-3").contains(&"Steam".to_owned()),
        "{text}"
    );
}

#[test]
fn wide_folder_names_keep_the_columns_lined_up_by_display_width() {
    use unicode_width::UnicodeWidthStr;

    let f = Fixture::new();
    f.global_config(on());
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");
    // Wider on screen than any other name, so it sets the column.
    let wide = "测试测试测试测试测试测试";
    let broken = f.external_dir("steam").join(wide);
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("proton"), "not executable").unwrap();

    let text = stdout(&f.run_at_home(&["runners", "--all"]));
    let source_at = |name: &str| {
        let line = text
            .lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_else(|| panic!("no row for {name} in:\n{text}"));
        line[..line.find("Steam").unwrap()].width()
    };
    assert_eq!(source_at(wide), source_at("GE-Proton11-3"), "{text}");
}

#[test]
fn a_root_inside_a_launcher_folder_is_labeled_with_that_launcher() {
    let f = Fixture::new();
    let steam = f.external_dir("steam");
    fs::create_dir_all(&steam).unwrap();
    f.global_config(&format!(
        "[defaults]\nrunner_root = '{}'\n",
        steam.display()
    ));
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");

    let text = stdout(&f.run_at_home(&["runners"]));
    assert_eq!(row(&text, "GE-Proton11-3")[2], "Steam", "{text}");
}

#[test]
fn the_import_review_names_a_launchers_build() {
    let f = Fixture::new();
    f.global_config(on());
    f.install_external_wine("lutris", "lutris-9");
    let prefix = f.root.path().join("old prefix");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();

    for (runner, shown) in [
        ("lutris-9", "runner: lutris-9 -> lutris-9 (Lutris)\n"),
        ("wine", "runner: wine -> wine\n"),
    ] {
        let destination = f.root.path().join(format!("copy-{runner}"));
        let imported = f.run_at_home(&[
            "import",
            "prefix",
            prefix.to_str().unwrap(),
            "-n",
            runner,
            "--runner",
            runner,
            "--dir",
            destination.to_str().unwrap(),
        ]);
        assert_success(&imported);
        assert!(stdout(&imported).contains(shown), "{}", stdout(&imported));
    }
}

#[test]
fn removing_a_launchers_build_says_whose_it_is_without_asking() {
    let f = Fixture::new();
    f.global_config(on());
    f.install_external_proton("steam", "GE-Proton11-3", "GE-Proton11-3");
    f.install_external_proton("heroic-proton", "GE-Proton11-1", "GE-Proton11-1");

    for (name, launcher) in [("GE-Proton11-3", "Steam"), ("GE-Proton11-1", "Heroic")] {
        let removed = f.run_at_home(&["runners", "remove", name, "--yes"]);
        assert!(!removed.status.success());
        assert_eq!(
            stderr(&removed).trim(),
            format!("error: {name} belongs to {launcher}."),
            "{}",
            stderr(&removed)
        );
        assert_eq!(stdout(&removed), "");
    }
    assert!(
        f.external_dir("steam")
            .join("GE-Proton11-3/proton")
            .is_file()
    );
    assert!(
        f.external_dir("heroic-proton")
            .join("GE-Proton11-1/proton")
            .is_file()
    );

    // A root inside a launcher folder is theirs too.
    let steam = f.external_dir("steam");
    f.global_config(&format!(
        "[defaults]\nrunner_root = '{}'\n",
        steam.display()
    ));
    let removed = f.run_at_home(&["runners", "remove", "GE-Proton11-3", "--yes"]);
    assert_eq!(
        stderr(&removed).trim(),
        "error: GE-Proton11-3 belongs to Steam."
    );
    assert!(steam.join("GE-Proton11-3/proton").is_file());
}
