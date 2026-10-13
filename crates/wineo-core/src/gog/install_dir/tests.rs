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
