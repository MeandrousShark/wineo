use std::fs;
use std::path::Path;

use super::*;

fn write(dir: &Path, file: &str, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(file), text).unwrap();
}

#[test]
fn names_come_from_the_folder_and_its_game_directory() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    write(
        &game,
        "goggame-1094900565.info",
        r#"{"gameId":"1094900565","name":"RimWorld","rootGameId":"1094900565"}"#,
    );
    write(
        &game,
        "goggame-1233017772.info",
        r#"{"gameId":"1233017772","name":"RimWorld - Royalty"}"#,
    );
    write(
        root.path(),
        "goggame-1827857764.info",
        r#"{"gameId":"1827857764","name":"RimWorld - Ideology"}"#,
    );
    let names = product_names(root.path());
    assert_eq!(names.len(), 3);
    assert_eq!(names[&1094900565], "RimWorld");
    assert_eq!(names[&1233017772], "RimWorld - Royalty");
    assert_eq!(names[&1827857764], "RimWorld - Ideology");
}

#[test]
fn a_numeric_game_id_counts() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "goggame-1207658924.info",
        r#"{"gameId":1207658924,"name":"Disco Elysium"}"#,
    );
    assert_eq!(product_names(root.path())[&1207658924], "Disco Elysium");
}

#[test]
fn a_file_naming_another_game_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "goggame-1.info",
        r#"{"gameId":"2","name":"Someone else"}"#,
    );
    write(root.path(), "goggame-3.info", r#"{"name":"No id"}"#);
    assert!(product_names(root.path()).is_empty());
}

#[test]
fn unreadable_files_are_skipped_without_hiding_the_rest() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "goggame-1.info", "not json");
    write(root.path(), "goggame-2.info", r#"{"gameId":"2","name":""}"#);
    write(root.path(), "goggame-3.info", r#"{"gameId":"3","name":7}"#);
    write(
        root.path(),
        "goggame-x.info",
        r#"{"gameId":"x","name":"X"}"#,
    );
    write(root.path(), "other.info", r#"{"gameId":"5","name":"Five"}"#);
    write(
        root.path(),
        "goggame-4.info",
        r#"{"gameId":"4","name":"Four"}"#,
    );
    // A directory named like an info file reads as an error, not a name.
    fs::create_dir(root.path().join("goggame-6.info")).unwrap();
    let names = product_names(root.path());
    assert_eq!(names.len(), 1);
    assert_eq!(names[&4], "Four");
}

#[test]
fn a_missing_folder_has_no_names() {
    let root = tempfile::tempdir().unwrap();
    assert!(product_names(&root.path().join("gone")).is_empty());
}

#[test]
fn the_folder_itself_wins_over_game() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "goggame-1.info",
        r#"{"gameId":"1","name":"Outer"}"#,
    );
    write(
        &root.path().join("game"),
        "goggame-1.info",
        r#"{"gameId":"1","name":"Inner"}"#,
    );
    assert_eq!(product_names(root.path())[&1], "Outer");
}

#[test]
fn a_label_drops_the_game_title_and_a_separator() {
    assert_eq!(
        dlc_label("RimWorld", 1233017772, Some("RimWorld - Royalty")),
        "Royalty (1233017772)"
    );
    assert_eq!(
        dlc_label(
            "Wasteland 3",
            7,
            Some("Wasteland 3: Cult of the Holy Detonation")
        ),
        "Cult of the Holy Detonation (7)"
    );
}

#[test]
fn a_label_keeps_a_name_it_cant_shorten() {
    // Nothing would be left.
    assert_eq!(
        dlc_label("RimWorld", 1, Some("RimWorld - ")),
        "RimWorld - (1)"
    );
    assert_eq!(dlc_label("RimWorld", 1, Some("RimWorld")), "RimWorld (1)");
    // The prefix isn't the title plus a separator.
    assert_eq!(
        dlc_label("RimWorld", 2, Some("RimWorldly Extras")),
        "RimWorldly Extras (2)"
    );
    assert_eq!(dlc_label("RimWorld", 3, Some("Ideology")), "Ideology (3)");
    assert_eq!(
        dlc_label("Rimworld", 4, Some("RimWorld - Biotech")),
        "RimWorld - Biotech (4)"
    );
    assert_eq!(dlc_label("", 5, Some(" - Odd")), "- Odd (5)");
}

#[test]
fn a_label_without_a_name_says_dlc() {
    assert_eq!(dlc_label("RimWorld", 9, None), "DLC (9)");
    assert_eq!(dlc_label("RimWorld", 9, Some("  ")), "DLC (9)");
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn a_fifo_named_like_an_info_file_is_skipped_without_waiting() {
    let root = tempfile::tempdir().unwrap();
    mkfifo(&root.path().join("goggame-1.info"));
    let game = root.path().join("game");
    fs::create_dir(&game).unwrap();
    mkfifo(&game.join("goggame-2.info"));
    std::os::unix::fs::symlink(
        root.path().join("goggame-1.info"),
        game.join("goggame-3.info"),
    )
    .unwrap();
    write(
        root.path(),
        "goggame-4.info",
        r#"{"gameId":"4","name":"Four"}"#,
    );
    let names = product_names(root.path());
    assert_eq!(names.len(), 1);
    assert_eq!(names[&4], "Four");
}

#[test]
fn a_symlink_to_a_regular_info_file_still_counts() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "real.json", r#"{"gameId":"5","name":"Five"}"#);
    std::os::unix::fs::symlink(
        root.path().join("real.json"),
        root.path().join("goggame-5.info"),
    )
    .unwrap();
    assert_eq!(product_names(root.path())[&5], "Five");
}

#[test]
fn an_oversized_info_file_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    let padding = " ".repeat(MAX_INFO_BYTES as usize);
    write(
        root.path(),
        "goggame-1.info",
        &format!(r#"{{"gameId":"1","name":"Big"}}{padding}"#),
    );
    // Just inside the cap still reads.
    let padding = " ".repeat(MAX_INFO_BYTES as usize - 28);
    write(
        root.path(),
        "goggame-2.info",
        &format!(r#"{{"gameId":"2","name":"Ok"}}{padding}"#),
    );
    let names = product_names(root.path());
    assert!(!names.contains_key(&1));
    assert_eq!(names[&2], "Ok");
}

#[test]
fn an_id_written_with_leading_zeros_is_not_the_files_id() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "goggame-01.info",
        r#"{"gameId":"1","name":"One"}"#,
    );
    assert!(product_names(root.path()).is_empty());
}
