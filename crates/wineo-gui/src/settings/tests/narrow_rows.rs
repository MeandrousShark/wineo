//! Settings rows in a narrow window and a wide one: a control that is fixed
//! wide or fills folds under its label rather than squeezing, and a wide
//! window lays the rows out as it always did. These lay out the real pages
//! and find each row by its label, so every kind of caller is covered.

use std::sync::{Arc, Mutex};

use iced::Rectangle;
use iced::advanced::Layout;
use iced::advanced::widget::{Id, Operation};

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::Palette;

const WIDE: f32 = 1000.0;
const NARROW: f32 = 360.0;
/// The label, the gap, and a control.
const BESIDE: f32 = 192.0;

/// The bounds of each child of every row labelled `label`, in the page laid
/// out `width` wide.
fn rows(page: &Page, paths: &Paths, label: &str, width: f32) -> Vec<Vec<Rectangle>> {
    struct Labels(Arc<Mutex<Vec<(Rectangle, String)>>>);
    impl Operation for Labels {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
            self.0.lock().unwrap().push((bounds, text.to_owned()));
        }
    }
    fn find(layout: Layout<'_>, label: Rectangle, found: &mut Vec<Vec<Rectangle>>) {
        let children: Vec<_> = layout.children().collect();
        if children.len() >= 2 && children[0].bounds() == label {
            found.push(children.iter().map(Layout::bounds).collect());
        }
        for child in children {
            find(child, label, found);
        }
    }
    let texts = Arc::new(Mutex::new(Vec::new()));
    let mut laid = lay_out(
        page.view(paths, Palette::DARK),
        iced::Size::new(width, 4000.0),
    );
    laid.run(Box::new(Labels(texts.clone())));
    let mut found = Vec::new();
    for (bounds, text) in texts.lock().unwrap().iter() {
        if text == label {
            find(Layout::new(&laid.node), *bounds, &mut found);
        }
    }
    assert!(!found.is_empty(), "no row labelled {label}");
    found
}

/// The control sits beside its label, as it always has, on a wide page.
fn assert_beside(rows: &[Vec<Rectangle>], label: &str) {
    for row in rows {
        let (name, control) = (row[0], row[1]);
        assert_eq!(control.x - name.x, BESIDE, "{label}");
        let middle = |bounds: Rectangle| bounds.y + bounds.height / 2.0;
        assert!((middle(name) - middle(control)).abs() < 0.6, "{label}");
    }
}

/// The control is under its label, at its left edge, and still wide enough
/// to use.
fn assert_folded(rows: &[Vec<Rectangle>], label: &str) {
    for row in rows {
        let (name, control) = (row[0], row[1]);
        assert_eq!(control.x, name.x, "{label}");
        assert!(control.y >= name.y + name.height, "{label}");
        assert!(control.width >= 220.0, "{label}: {}", control.width);
    }
}

fn global(paths: &Paths) -> Page {
    global_page(paths)
}

/// An environment on GE-Proton with a DXVK build set, which core refuses,
/// so the Builds row has a Remove button beside its control.
fn refused_build(paths: &Paths) -> Page {
    fake_runners(paths);
    write(
        paths,
        &GAME
            .replace("\"missing\"", "\"GE-Proton9-20\"")
            .replace("[gamescope]", "dxvk = \"latest\"\n\n[gamescope]"),
    );
    page(paths)
}

/// An environment with a gamescope table and a target that has one too, its
/// card open, so both the plain and the indented fields show.
fn gamescope(paths: &Paths) -> Page {
    write(
        paths,
        &format!(
            "{GAME}\n[targets.game]\nexe = \"/bin/game\"\n[targets.game.gamescope]\nwidth = 5\n"
        ),
    );
    let mut page = page(paths);
    let _ = page.update(paths, Event::Card("game".to_owned(), true));
    page
}

#[test]
fn global_rows_with_a_fixed_control_fold_in_a_narrow_window() {
    let (_root, paths) = setup();
    let page = global(&paths);
    for label in [
        "Runner",
        "Arch",
        "Closing the window",
        "When something starts",
        "When everything stops",
    ] {
        assert_beside(&rows(&page, &paths, label, WIDE), label);
        assert_folded(&rows(&page, &paths, label, NARROW), label);
    }
}

#[test]
fn a_locked_control_folds_like_the_open_one() {
    let (_root, paths) = setup();
    let mut page = global(&paths);
    // Turning the tray off locks the close choice.
    let _ = page.update(&paths, Event::GlobalTray(false));
    let label = "Closing the window";
    assert_beside(&rows(&page, &paths, label, WIDE), label);
    assert_folded(&rows(&page, &paths, label, NARROW), label);
    // So does a page that is saving, which locks every control.
    page.saving = true;
    for label in ["Runner", "Arch", "Closing the window"] {
        assert_beside(&rows(&page, &paths, label, WIDE), label);
        assert_folded(&rows(&page, &paths, label, NARROW), label);
    }
}

#[test]
fn a_field_that_fills_folds_when_it_would_get_less_than_a_control_width() {
    let (_root, paths) = setup();
    let page = global(&paths);
    // The games folder has a Browse button beside its field.
    let label = "Games folder";
    let wide = rows(&page, &paths, label, WIDE);
    assert_beside(&wide, label);
    for row in &wide {
        let (field, browse) = (row[1], row[2]);
        assert!(field.width >= 260.0, "{}", field.width);
        assert!(browse.x >= field.x + field.width, "Browse sits after it");
    }
    for row in rows(&page, &paths, label, NARROW) {
        let (name, field, browse) = (row[0], row[1], row[2]);
        assert_eq!(field.x, name.x);
        assert!(field.y >= name.y + name.height);
        // The button follows the field, or goes under it when there is no
        // room, and never leaves the field squeezed.
        assert!(field.width >= 220.0, "{}", field.width);
        assert!(browse.x >= field.x + field.width || browse.y >= field.y + field.height);
    }
}

#[test]
fn a_filling_field_beside_its_label_keeps_a_wide_window_unchanged() {
    let (_root, paths) = setup();
    let page = global(&paths);
    // 1000 wide leaves 728 for a row, and 536 for the field after the label
    // and the Browse button.
    let row = &rows(&page, &paths, "Games folder", WIDE)[0];
    let room = row[2].x + row[2].width - row[0].x;
    assert!((room - 728.0).abs() < 0.01, "{room}");
    assert_eq!(row[1].x - row[0].x, BESIDE);
    assert_eq!(row[2].x, row[1].x + row[1].width + 12.0);
}

#[test]
fn an_environments_title_field_folds() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let page = page(&paths);
    let label = "Title";
    assert_beside(&rows(&page, &paths, label, WIDE), label);
    assert_folded(&rows(&page, &paths, label, NARROW), label);
}

#[test]
fn the_proxied_title_note_goes_under_its_control() {
    let (_root, paths) = setup();
    write(
        &paths,
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\n",
    );
    // The locked title is a fixed control with a note after it.
    let page = page(&paths);
    let row = |width| rows(&page, &paths, "Title", width).remove(0);
    let wide = row(WIDE);
    assert_eq!(wide.len(), 3);
    assert_eq!(wide[2].x, wide[1].x + wide[1].width + 12.0, "one line");
    assert!((wide[2].y - wide[1].y).abs() < 20.0);
    // Room for the label and control but not the note after them.
    let medium = row(580.0);
    assert_eq!(medium[1].x - medium[0].x, BESIDE);
    assert_eq!(medium[2].x, medium[1].x, "under the control");
    assert!(medium[2].y >= medium[1].y + medium[1].height);
    // Too narrow for the control beside the label.
    let narrow = row(NARROW);
    assert_eq!(narrow[1].x, narrow[0].x);
    assert_eq!(narrow[2].x, narrow[0].x, "after the control, at the edge");
    assert!(narrow[1].y >= narrow[0].y + narrow[0].height);
    assert!(narrow[2].y >= narrow[1].y + narrow[1].height);
}

#[test]
fn the_builds_row_note_follows_its_control() {
    let (_root, paths) = setup();
    let page = refused_build(&paths);
    let row = |width| rows(&page, &paths, "DXVK", width).remove(0);
    let wide = row(808.0);
    assert_eq!(wide.len(), 3);
    assert_eq!(wide[1].x - wide[0].x, BESIDE);
    assert_eq!(wide[2].x, wide[1].x + wide[1].width + 12.0, "one line");
    assert!(wide[2].y < wide[1].y + wide[1].height);
    let medium = row(580.0);
    assert_eq!(medium[2].x, medium[1].x, "under the control");
    assert!(medium[2].y >= medium[1].y + medium[1].height);
    let narrow = row(NARROW);
    assert_eq!(narrow[1].x, narrow[0].x);
    assert_eq!(narrow[2].x, narrow[0].x);
    assert!(narrow[1].y >= narrow[0].y + narrow[0].height);
}

#[test]
fn the_needs_dxvk_note_folds_with_the_vkd3d_control() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let page = page(&paths);
    let label = "vkd3d-proton";
    let wide = rows(&page, &paths, label, WIDE);
    assert_beside(&wide, label);
    let narrow = rows(&page, &paths, label, NARROW);
    assert_folded(&narrow, label);
    assert_eq!(narrow[0][2].x, narrow[0][0].x);
}

#[test]
fn gamescope_fields_fold_like_the_other_rows() {
    let (_root, paths) = setup();
    let page = gamescope(&paths);
    // A choice and a text field, in the environment and in the target card,
    // which is indented.
    for label in ["Filter", "Width"] {
        let wide = rows(&page, &paths, label, WIDE);
        assert_eq!(wide.len(), 2, "{label} in both places");
        for row in &wide {
            assert_eq!(row[1].x - row[0].x, 168.0, "{label}");
        }
        for row in rows(&page, &paths, label, NARROW) {
            assert_eq!(row[1].x, row[0].x, "{label}");
            assert!(row[1].y >= row[0].y + row[0].height, "{label}");
            assert!(row[1].width >= 200.0, "{label}: {}", row[1].width);
        }
    }
}

#[test]
fn a_target_cards_exe_field_folds_with_its_browse_button() {
    let (_root, paths) = setup();
    let page = gamescope(&paths);
    let label = "Exe";
    for row in rows(&page, &paths, label, WIDE) {
        assert_eq!(row[1].x - row[0].x, BESIDE);
    }
    for row in rows(&page, &paths, label, NARROW) {
        assert_eq!(row[1].x, row[0].x);
        assert!(row[1].y >= row[0].y + row[0].height);
        assert!(row[1].width >= 200.0, "{}", row[1].width);
    }
}
