use super::*;

const SEARCH: Search = Search {
    min: 160.0,
    max: 320.0,
    icon: 32.0,
};

fn item(control: Option<Control>, width: f32) -> Item {
    Item { control, width }
}

/// Sort, Filters, the dice, the layout switch, the slider, and the toggle,
/// at their widths in the toolbar.
fn toolbar() -> Vec<Item> {
    vec![
        item(None, 160.0),
        item(Some(Control::Filters), 86.0),
        item(Some(Control::Dice), 32.0),
        item(Some(Control::Layout), 64.0),
        item(Some(Control::Zoom), 96.0),
        item(Some(Control::Titles), 32.0),
    ]
}

/// The widths and gaps after search with nothing folded.
const ALL: f32 = 160.0 + 86.0 + 32.0 + 64.0 + 96.0 + 32.0 + 6.0 * 8.0;

fn squeeze(room: f32) -> Squeeze {
    plan(room, SEARCH, &toolbar(), 32.0, 8.0)
}

fn folded(controls: &[Control]) -> Folds {
    controls
        .iter()
        .fold(Folds::default(), |folds, control| folds.with(*control))
}

#[test]
fn search_takes_what_is_left_up_to_its_width_while_nothing_folds() {
    assert_eq!(
        squeeze(ALL + 400.0),
        Squeeze {
            folds: Folds::default(),
            search: 320.0,
            search_icon: false,
        }
    );
    assert_eq!(squeeze(ALL + 200.0).search, 200.0);
    assert_eq!(
        squeeze(ALL + 160.0),
        Squeeze {
            folds: Folds::default(),
            search: 160.0,
            search_icon: false,
        }
    );
}

#[test]
fn controls_fold_right_to_left_only_while_the_row_still_does_not_fit() {
    // A pixel short: the toggle is no wider than More, so folding it alone
    // doesn't help, and zoom goes with it.
    let tight = squeeze(ALL + 159.0);
    assert_eq!(tight.folds, folded(&[Control::Titles, Control::Zoom]));
    assert_eq!(tight.search, 160.0);
    // The toggle and zoom out leave room for More plus 104 px.
    let after_zoom = ALL + 160.0 - 96.0 - 8.0;
    assert_eq!(squeeze(after_zoom).folds, tight.folds);
    assert_eq!(
        squeeze(after_zoom - 1.0).folds,
        folded(&[Control::Titles, Control::Zoom, Control::Layout])
    );
    let order = squeeze(0.0).folds.iter().collect::<Vec<_>>();
    assert_eq!(order, Control::ALL, "in toolbar order");
}

/// Sort and More with their gaps, all that's left once everything folds.
const FOLDED: f32 = 8.0 + 160.0 + 8.0 + 32.0;

#[test]
fn search_collapses_to_its_icon_only_after_the_last_fold() {
    // Everything folded and the field at its floor just fits.
    let fits = squeeze(160.0 + FOLDED);
    assert_eq!(fits.folds.iter().count(), Control::ALL.len());
    assert!(!fits.search_icon);
    assert_eq!(fits.search, 160.0);
    // A pixel less and search is its icon.
    let icon = squeeze(160.0 + FOLDED - 1.0);
    assert_eq!(icon.folds, fits.folds);
    assert!(icon.search_icon);
    assert_eq!(icon.search, 32.0);
    // Past that the right end clips, with search still an icon.
    assert!(squeeze(10.0).search_icon);
    // Never while a fold could still help.
    assert!(!squeeze(160.0 + FOLDED + 86.0 + 8.0).search_icon);
}

#[test]
fn an_absent_control_neither_folds_nor_takes_room() {
    // The list: no zoom or toggle.
    let items: Vec<Item> = toolbar()
        .into_iter()
        .filter(|item| !matches!(item.control, Some(Control::Zoom | Control::Titles)))
        .collect();
    let all = 160.0 + 86.0 + 32.0 + 64.0 + 4.0 * 8.0;
    let fits = plan(all + 160.0, SEARCH, &items, 32.0, 8.0);
    assert!(fits.folds.is_empty());
    let tight = plan(all + 159.0, SEARCH, &items, 32.0, 8.0);
    assert_eq!(tight.folds, folded(&[Control::Layout]));
}
