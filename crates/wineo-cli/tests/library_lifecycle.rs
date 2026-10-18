mod common;

use std::fs;

use common::{Fixture, assert_success, stderr};

#[test]
fn broken_library_warnings_reach_cli_for_new_clone_remove_and_restore() {
    let f = Fixture::new();
    fs::create_dir_all(f.config_dir()).unwrap();
    let library = f.config_dir().join("library.toml");
    let broken = "[broken\n";
    fs::write(&library, broken).unwrap();
    for args in [
        vec!["new", "-n", "native", "--native"],
        vec!["clone", "native", "copy"],
        vec!["rm", "native"],
    ] {
        let output = f.run(&args);
        assert_success(&output);
        assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
        assert!(
            stderr(&output).contains("TOML parse error"),
            "{}",
            stderr(&output)
        );
        assert_eq!(fs::read_to_string(&library).unwrap(), broken);
    }
    let trash = f.root.path().join("data/wineo/trash");
    let id = fs::read_dir(&trash)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    // Nothing was saved at removal, so restore leaves the broken file alone
    // and has nothing to warn about.
    let restored = f.run(&["restore", id.to_str().unwrap()]);
    assert_success(&restored);
    assert!(
        !stderr(&restored).contains("library.toml"),
        "{}",
        stderr(&restored)
    );
    assert_eq!(fs::read_to_string(&library).unwrap(), broken);

    // An entry saved at removal can't be put back into a broken file.
    fs::remove_file(&library).unwrap();
    assert_success(&f.run(&["new", "-n", "saved", "--native"]));
    assert_success(&f.run(&["rm", "saved"]));
    let id = fs::read_dir(&trash)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .find(|name| name != &id)
        .unwrap();
    fs::write(&library, broken).unwrap();
    let restored = f.run(&["restore", id.to_str().unwrap()]);
    assert_success(&restored);
    assert!(
        stderr(&restored).contains("warning:"),
        "{}",
        stderr(&restored)
    );
    assert!(
        stderr(&restored).contains("TOML parse error"),
        "{}",
        stderr(&restored)
    );
    assert_eq!(fs::read_to_string(&library).unwrap(), broken);
}

#[test]
fn broken_library_warning_reaches_cli_for_wine_creation() {
    let f = Fixture::new();
    fs::create_dir_all(f.config_dir()).unwrap();
    let library = f.config_dir().join("library.toml");
    let broken = "[broken\n";
    fs::write(&library, broken).unwrap();
    let output = f.run(&["new", "-n", "wine", "--runner", "wine"]);
    assert_success(&output);
    assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("TOML parse error"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read_to_string(&library).unwrap(), broken);
}

#[test]
fn restore_warning_names_library_values_on_cli() {
    let f = Fixture::new();
    assert_success(&f.run(&["new", "-n", "game", "--native"]));
    let library = f.config_dir().join("library.toml");
    fs::write(
        &library,
        "[environments.game]\nfavorite = true\nhidden = true\nadded = 2026-10-17T18:00:00Z\n",
    )
    .unwrap();
    assert_success(&f.run(&["rm", "game"]));
    let trash = f.root.path().join("data/wineo/trash");
    let id = fs::read_dir(trash)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    fs::write(&library, "[broken\n").unwrap();
    let restored = f.run(&["restore", id.to_str().unwrap()]);
    assert_success(&restored);
    assert!(
        stderr(&restored).contains("favorite, hidden, added 2026-10-17"),
        "{}",
        stderr(&restored)
    );
    assert!(
        stderr(&restored).contains("TOML parse error"),
        "{}",
        stderr(&restored)
    );
}
