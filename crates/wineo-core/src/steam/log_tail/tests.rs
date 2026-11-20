use super::*;
use std::io::Write;

fn lines(follower: &mut Follower) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut restarts = 0;
    follower
        .read(|event| match event {
            Event::Restarted => restarts += 1,
            Event::Line(line) => lines.push(line.to_owned()),
        })
        .unwrap();
    (lines, restarts)
}

fn append(path: &std::path::Path, text: &str) {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn a_stamp_and_a_line_ending_come_off_a_line() {
    assert_eq!(message("[2026-10-30 11:11:45] hello\r"), "hello");
    assert_eq!(message("hello"), "hello");
}

#[test]
fn only_whole_lines_written_since_the_last_read_are_given() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    append(&path, "old\n");
    let mut follower = Follower::at_end(path.clone());
    assert_eq!(lines(&mut follower), (vec![], 0));
    append(&path, "one\ntwo\nthr");
    assert_eq!(lines(&mut follower), (vec!["one".into(), "two".into()], 0));
    append(&path, "ee\n");
    assert_eq!(lines(&mut follower), (vec!["three".into()], 0));
}

#[test]
fn a_big_gap_is_read_in_pieces_and_a_line_across_a_piece_is_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut follower = Follower::at_end(path.clone());
    // Lines of 2 and 5 bytes, so some cross a chunk boundary.
    let text = "y
zzzz
"
    .repeat(CHUNK / 4);
    append(&path, &text);
    let (read, _) = lines(&mut follower);
    assert_eq!(read.len(), 2 * (CHUNK / 4));
    assert!(read.iter().all(|line| line == "y" || line == "zzzz"));
}

#[test]
fn a_line_longer_than_a_chunk_is_skipped_and_the_next_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    // The position lands inside a line, which is longer than a chunk.
    append(&path, "start");
    let mut follower = Follower::at_end(path.clone());
    append(&path, &"z".repeat(2 * CHUNK + 5));
    append(&path, "\nnext\n");
    assert_eq!(lines(&mut follower), (vec!["next".into()], 0));
}

#[test]
fn a_replaced_or_shortened_log_restarts_from_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    append(&path, "aaaa\nbbbb\n");
    let mut follower = Follower::at_end(path.clone());
    fs::write(&path, "c\n").unwrap();
    assert_eq!(lines(&mut follower), (vec!["c".into()], 1));
    append(&path, "d\n");
    assert_eq!(lines(&mut follower), (vec!["d".into()], 0));
    fs::remove_file(&path).unwrap();
    assert_eq!(lines(&mut follower), (vec![], 1));
    append(&path, "e\n");
    assert_eq!(lines(&mut follower), (vec!["e".into()], 1));
}

#[test]
fn a_line_longer_than_a_chunk_is_skipped_from_a_line_start_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut follower = Follower::at_end(path.clone());
    append(&path, &"z".repeat(2 * CHUNK + 5));
    assert_eq!(lines(&mut follower), (vec![], 1));
    append(&path, "\nnext\n");
    assert_eq!(lines(&mut follower), (vec!["next".into()], 0));
}
