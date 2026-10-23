//! Keyboard focus on the program list a finished Windows install offers.

use std::ffi::OsString;

use super::tests::rimworld;
use super::*;
use crate::focus_button::walk_rows;
use crate::layout_probe::lay_out;

/// An install that has just finished with `programs` to pick from.
fn finished(programs: &[&str]) -> (Install, Option<ListRows>) {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let (mut page, _) = Install::new(paths, rimworld());
    page.stage = Stage::Working;
    let before = page.rows();
    let done = Done {
        environment: "rimworld".to_owned(),
        programs: Some(
            programs
                .iter()
                .map(|exe| ((*exe).to_owned(), format!("C:\\Game\\{exe}")))
                .collect(),
        ),
        ..Done::default()
    };
    let generation = page.generation;
    let _ = page.update(Event::Finished(generation, Ok(Box::new(done))));
    assert_eq!(page.stage, Stage::Target);
    (page, before)
}

#[test]
fn the_program_list_opens_and_the_arrows_walk_it() {
    let (page, before) = finished(&["game.exe", "editor.exe", "server.exe"]);
    let ids: Vec<String> = ["game.exe", "editor.exe", "server.exe"]
        .iter()
        .map(|exe| program_id(&format!("C:\\Game\\{exe}")))
        .collect();
    // Coming from the working step, the list opens, so its first row takes
    // focus.
    assert_eq!(before, None);
    assert_eq!(
        page.rows(),
        Some(ListRows {
            list: PROGRAM_LIST,
            ids: ids.clone(),
        })
    );
    let (body, _) = page.view(iced::widget::column![], Palette::DARK);
    let mut laid = lay_out(body.into(), iced::Size::new(600.0, 800.0));
    walk_rows(&mut laid, &ids);
}

#[test]
fn nothing_installed_focuses_no_row() {
    let (page, _) = finished(&[]);
    assert_eq!(
        page.rows(),
        Some(ListRows {
            list: PROGRAM_LIST,
            ids: Vec::new(),
        })
    );
    let (body, _) = page.view(iced::widget::column![], Palette::DARK);
    let mut laid = lay_out(body.into(), iced::Size::new(600.0, 800.0));
    let rows = crate::focus_button::focusables(&mut laid);
    assert!(
        rows.iter().flatten().all(|id| !id.contains("gog-program")),
        "{rows:?}"
    );
    assert!(laid.focused().is_empty());
}
