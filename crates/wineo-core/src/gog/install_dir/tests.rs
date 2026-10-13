use super::*;

#[test]
fn a_plain_title_is_kept() {
    assert_eq!(
        safe_folder_name("Disco Elysium - The Final Cut").unwrap(),
        "Disco Elysium - The Final Cut"
    );
    assert_eq!(
        safe_folder_name("S.T.A.L.K.E.R.: Shadow").unwrap(),
        "S.T.A.L.K.E.R.: Shadow"
    );
}

#[test]
fn path_separators_are_removed() {
    assert_eq!(safe_folder_name("AC/DC Rocks").unwrap(), "ACDC Rocks");
    assert_eq!(safe_folder_name("A\\B").unwrap(), "AB");
    assert_eq!(safe_folder_name("../../etc").unwrap(), "....etc");
}

#[test]
fn control_characters_are_removed() {
    assert_eq!(
        safe_folder_name("Two\nLines\t\0Here\u{7f}").unwrap(),
        "TwoLinesHere"
    );
}

#[test]
fn direction_and_format_characters_are_removed() {
    assert_eq!(
        safe_folder_name("Game\u{202e}gpj.exe\u{202c}").unwrap(),
        "Gamegpj.exe"
    );
    assert_eq!(
        safe_folder_name("\u{200e}A\u{200f}B\u{2066}C\u{2069}\u{200b}D\u{200d}E\u{feff}").unwrap(),
        "ABCDE"
    );
    assert!(safe_folder_name("\u{202e}\u{200b}").is_err());
    for invisible in [
        "\u{600}",
        "\u{6dd}",
        "\u{110bd}",
        "\u{13430}",
        "\u{1bca0}\u{1bca3}",
        "\u{1d173}",
        "\u{e0001}\u{e0020}\u{e007f}",
    ] {
        assert!(safe_folder_name(invisible).is_err(), "{invisible:?}");
        assert_eq!(
            safe_folder_name(&format!("Game{invisible}")).unwrap(),
            "Game"
        );
    }
    // Just outside the ranges.
    assert_eq!(
        safe_folder_name("\u{e0000}\u{e0080}").unwrap(),
        "\u{e0000}\u{e0080}"
    );
    // Other Unicode stays, the fullwidth slash included.
    let kept = "Caf\u{e9} \u{ff0f} \u{4e2d}\u{6587}";
    assert_eq!(safe_folder_name(kept).unwrap(), kept);
}

#[test]
fn trailing_dots_and_spaces_are_removed() {
    assert_eq!(safe_folder_name("Game. . ").unwrap(), "Game");
    assert_eq!(safe_folder_name("Game...").unwrap(), "Game");
    assert_eq!(safe_folder_name(" Game").unwrap(), " Game");
    // Removal can expose more to trim.
    assert_eq!(safe_folder_name("Title /").unwrap(), "Title");
    assert_eq!(safe_folder_name("Title. /.").unwrap(), "Title");
}

#[test]
fn a_title_with_nothing_left_is_refused() {
    for title in ["", ".", "..", "...", " . ", "/", "\u{1}\u{2}"] {
        let error = safe_folder_name(title).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("{title:?} can't be made into a folder name.")
        );
    }
}

#[test]
fn the_folder_is_the_title_under_the_games_root_unless_overridden() {
    let root = Path::new("/home/me/Games");
    assert_eq!(
        install_folder(root, "RimWorld", None).unwrap(),
        Path::new("/home/me/Games/RimWorld")
    );
    assert_eq!(
        install_folder(root, "AC/DC", None).unwrap(),
        Path::new("/home/me/Games/ACDC")
    );
    assert_eq!(
        install_folder(root, "RimWorld", Some(Path::new("/mnt/games/rw"))).unwrap(),
        Path::new("/mnt/games/rw")
    );
    // The override is taken as given, so a bad title doesn't matter with it.
    assert_eq!(
        install_folder(root, "...", Some(Path::new("/mnt/x"))).unwrap(),
        Path::new("/mnt/x")
    );
    assert!(install_folder(root, "...", None).is_err());
}
