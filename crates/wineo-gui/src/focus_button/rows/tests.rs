use iced::advanced::widget::operation::focusable;
use iced::event::Status;
use iced::keyboard::Modifiers;
use iced::widget::{column, text_input};

use super::*;
use crate::layout_probe::{Laid, lay_out, relay_out};
use crate::theme::Palette;
use crate::ui::{ListRow, list_group, list_row, list_row_text};

const LIST: &str = "test-list";

/// Presses `key` with `modifiers` and returns what it published and whether
/// a widget kept it.
pub(crate) fn press<M>(
    laid: &mut Laid<'_, M>,
    key: Named,
    modifiers: Modifiers,
) -> (Vec<M>, Status) {
    press_key(laid, Key::Named(key), modifiers)
}

pub(crate) fn press_key<M>(
    laid: &mut Laid<'_, M>,
    key: Key,
    modifiers: Modifiers,
) -> (Vec<M>, Status) {
    laid.event_with_status(
        &Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }),
        mouse::Cursor::Unavailable,
    )
}

fn focus<M>(laid: &mut Laid<'_, M>, id: &str) {
    laid.run(Box::new(focusable::focus::<()>(Id::from(id.to_owned()))));
}

/// Lays out the window and runs the widget operations `task` asks for
/// right away, as the runtime does after an update.
pub(crate) fn landed(app: &crate::App, task: Task<crate::Message>) -> Laid<'_, crate::Message> {
    let mut laid = lay_out(app.view(), Size::new(1200.0, 800.0));
    run_task(&mut laid, task);
    laid
}

/// Runs the widget operations `task` asks for right away on `laid`.
pub(crate) fn run_task<M, T: Send + 'static>(laid: &mut Laid<'_, M>, task: Task<T>) {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return;
    };
    iced::futures::executor::block_on(async {
        while let Some(Some(action)) = iced::futures::FutureExt::now_or_never(stream.next()) {
            if let iced_runtime::Action::Widget(operation) = action {
                laid.run(operation);
            }
        }
    });
}

/// Every focusable's id in tree order, with None for one without an id.
pub(crate) fn focusables<M>(laid: &mut Laid<'_, M>) -> Vec<Option<String>> {
    use std::sync::{Arc, Mutex};
    struct All(Arc<Mutex<Vec<Option<String>>>>);
    impl Operation for All {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
            self.0.lock().unwrap().push(id.map(|id| format!("{id:?}")));
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(All(found.clone())));
    found.lock().unwrap().clone()
}

/// Focuses the first of `ids`, which a laid out page shows as one list in
/// that order, then walks it with Down and back with Up, checking each key
/// is kept, chooses nothing, and stops at the ends.
pub(crate) fn walk_rows<M: std::fmt::Debug>(laid: &mut Laid<'_, M>, ids: &[String]) {
    let first = ids.first().expect("rows to walk");
    focus(laid, first);
    assert_eq!(laid.focused(), [Id::from(first.clone())]);
    let mut step = |key, expected: &String| {
        let (messages, status) = press(laid, key, Modifiers::empty());
        assert!(messages.is_empty(), "{key:?} chose {messages:?}");
        assert_eq!(status, Status::Captured, "{key:?}");
        assert_eq!(laid.focused(), [Id::from(expected.clone())], "{key:?}");
    };
    for id in &ids[1..] {
        step(Named::ArrowDown, id);
    }
    step(Named::ArrowDown, ids.last().unwrap());
    for id in ids.iter().rev().skip(1) {
        step(Named::ArrowUp, id);
    }
    step(Named::ArrowUp, first);
}

/// A list of `rows`, each named by its id and chosen by a message of the
/// same text, with a field under it.
fn view<'a>(list: &'static str, rows: &[(&'a str, bool)]) -> Element<'a, String> {
    column![
        list_group(
            list,
            rows.iter().map(|(id, enabled)| {
                ListRow::new(
                    *id,
                    list_row(list_row_text(*id), Palette::DARK),
                    (*id).to_owned(),
                )
                .enabled(*enabled)
            }),
            Palette::DARK,
        ),
        text_input("Field", "")
            .id(Id::new("field"))
            .on_input(|value| value),
    ]
    .into()
}

fn laid<'a>(rows: &[(&'a str, bool)]) -> Laid<'a, String> {
    lay_out(view(LIST, rows), Size::new(400.0, 600.0))
}

fn ids(names: &[&str]) -> Vec<Id> {
    names
        .iter()
        .map(|name| Id::from((*name).to_owned()))
        .collect()
}

fn listed(list: &'static str, ids: &[&str]) -> ListRows {
    ListRows {
        list,
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

#[test]
fn up_and_down_move_between_rows_and_stop_at_the_ends() {
    let mut laid = laid(&[("a", true), ("b", true), ("c", true)]);
    walk_rows(&mut laid, &["a", "b", "c"].map(str::to_owned));
}

#[test]
fn arrows_are_left_alone_when_no_row_has_focus() {
    let mut laid = laid(&[("a", true), ("b", true)]);
    for key in [Named::ArrowDown, Named::ArrowUp] {
        assert_eq!(press(&mut laid, key, Modifiers::empty()).1, Status::Ignored);
    }
    // Typing in the field below keeps the arrows too.
    focus(&mut laid, "field");
    let (_, status) = press(&mut laid, Named::ArrowUp, Modifiers::empty());
    assert_eq!(status, Status::Ignored);
    assert_eq!(laid.focused(), [Id::new("field")]);
    // Ctrl and an arrow are someone else's shortcut.
    focus(&mut laid, "a");
    let (_, status) = press(&mut laid, Named::ArrowDown, Modifiers::CTRL);
    assert_eq!(status, Status::Ignored);
    assert_eq!(laid.focused(), ids(&["a"]));
}

#[test]
fn enter_and_space_choose_the_focused_row_but_ctrl_enter_does_not() {
    let mut laid = laid(&[("a", true), ("b", true)]);
    focus(&mut laid, "b");
    for key in [Named::Enter, Named::Space] {
        let (messages, status) = press(&mut laid, key, Modifiers::empty());
        assert_eq!(messages, ["b"], "{key:?}");
        assert_eq!(status, Status::Captured);
    }
    let (messages, status) = press(&mut laid, Named::Enter, Modifiers::CTRL);
    assert!(messages.is_empty());
    assert_eq!(status, Status::Ignored, "the flow's submit gets it");
    // Only Enter is the submit; Ctrl+Space still chooses, like any control.
    let (messages, _) = press(&mut laid, Named::Space, Modifiers::CTRL);
    assert_eq!(messages, ["b"]);
}

#[test]
fn a_row_that_cant_be_chosen_takes_no_focus_and_is_stepped_over() {
    let mut laid = laid(&[("a", true), ("b", false), ("c", true)]);
    focus(&mut laid, "b");
    assert!(laid.focused().is_empty());
    let (messages, _) = press(&mut laid, Named::Enter, Modifiers::empty());
    assert!(messages.is_empty());
    focus(&mut laid, "a");
    press(&mut laid, Named::ArrowDown, Modifiers::empty());
    assert_eq!(laid.focused(), ids(&["c"]));
    press(&mut laid, Named::ArrowUp, Modifiers::empty());
    assert_eq!(laid.focused(), ids(&["a"]));
}

/// Lays out `before` with `focused` focused, then the list as `after`,
/// diffed like the runtime does, and returns what has focus.
fn refocused(before: &[&str], focused: &str, list: &'static str, after: &[&str]) -> Vec<Id> {
    let rows: Vec<_> = before.iter().map(|id| (*id, true)).collect();
    let mut laid = laid(&rows);
    focus(&mut laid, focused);
    let rows: Vec<_> = after.iter().map(|id| (*id, true)).collect();
    let mut laid = relay_out(view(list, &rows), laid.into_tree(), Size::new(400.0, 600.0));
    laid.focused()
}

#[test]
fn focus_stays_on_its_row_when_the_list_changes_around_it() {
    // Moved down by a new row above, and up by one removed above.
    assert_eq!(
        refocused(&["a", "b", "c"], "b", LIST, &["x", "a", "b", "c"]),
        ids(&["b"])
    );
    assert_eq!(
        refocused(&["a", "b", "c"], "c", LIST, &["b", "c"]),
        ids(&["c"])
    );
}

#[test]
fn focus_moves_to_the_nearest_row_when_its_row_goes() {
    assert_eq!(
        refocused(&["a", "b", "c"], "b", LIST, &["a", "c"]),
        ids(&["c"])
    );
    // The last row goes: the new last one, not a row past the end.
    assert_eq!(
        refocused(&["a", "b", "c"], "c", LIST, &["a", "b"]),
        ids(&["b"])
    );
    // A whole new list, like another branch's builds, keeps the place.
    assert_eq!(
        refocused(&["a", "b", "c"], "b", LIST, &["x", "y", "z"]),
        ids(&["y"])
    );
    assert_eq!(refocused(&["a", "b", "c"], "c", LIST, &["x"]), ids(&["x"]));
}

#[test]
fn an_empty_list_or_another_list_focuses_no_row() {
    assert!(refocused(&["a", "b"], "b", LIST, &[]).is_empty());
    assert!(refocused(&["a", "b"], "a", "other-list", &["a", "b"]).is_empty());
}

#[test]
fn nearest_skips_rows_that_cant_take_focus() {
    let rows = [
        (Id::new("x"), false),
        (Id::new("y"), false),
        (Id::new("z"), true),
    ];
    assert_eq!(follow(&ids(&["a", "b"]), 0, &rows), Some(2));
    assert_eq!(follow(&ids(&["a"]), 0, &rows[..2]), None);
    assert_eq!(follow(&ids(&["a"]), 0, &[]), None);
}

#[test]
fn a_list_that_opens_focuses_its_first_row_and_one_that_fills_late_waits() {
    let add = listed("add", &["add-new", "add-gog"]);
    assert_eq!(landing(None, Some(&add)), Landing::Focus("add-new".into()));
    let programs = listed("programs", &["p1", "p2"]);
    assert_eq!(
        landing(Some(&add), Some(&programs)),
        Landing::Focus("p1".into())
    );
    let empty = listed("programs", &[]);
    assert_eq!(
        landing(Some(&empty), Some(&programs)),
        Landing::FocusIfIdle("p1".into())
    );
    // Already shown, or gone, or empty: the page's focus stays as it is.
    assert_eq!(landing(Some(&programs), Some(&programs)), Landing::Stay);
    let fewer = listed("programs", &["p2"]);
    assert_eq!(landing(Some(&programs), Some(&fewer)), Landing::Stay);
    assert_eq!(landing(Some(&programs), None), Landing::Stay);
    assert_eq!(landing(None, Some(&empty)), Landing::Stay);
}

#[test]
fn a_late_list_takes_focus_only_when_nothing_has_it() {
    let mut laid = laid(&[("a", true), ("b", true)]);
    laid.run(Box::new(FocusIfIdle {
        id: Id::new("a"),
        busy: false,
    }));
    assert_eq!(laid.focused(), ids(&["a"]));

    let mut laid = self::laid(&[("a", true), ("b", true)]);
    focus(&mut laid, "field");
    laid.run(Box::new(FocusIfIdle {
        id: Id::new("a"),
        busy: false,
    }));
    assert_eq!(laid.focused(), [Id::new("field")]);
}

const VIEWPORT: f32 = 150.0;
const SCROLL_ID: &str = "test-scroll";

/// `names` as one list under a short heading, in a scrollable `height` tall.
fn scrolled<'a>(names: &[String], height: f32) -> Element<'a, String> {
    let rows: Vec<_> = names
        .iter()
        .map(|id| {
            ListRow::new(
                id.clone(),
                list_row(list_row_text(id.clone()), Palette::DARK),
                id.clone(),
            )
        })
        .collect();
    iced::widget::scrollable(column![
        iced::widget::text("Heading").size(20),
        list_group(LIST, rows, Palette::DARK)
    ])
    .id(Id::new(SCROLL_ID))
    .height(height)
    .into()
}

fn names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

/// The row's top and bottom in the scrolled content.
fn span<M>(laid: &mut Laid<'_, M>, id: &str) -> (f32, f32) {
    use std::sync::{Arc, Mutex};
    let found = Arc::new(Mutex::new(None));
    let sink = found.clone();
    let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
    laid.run(Box::new(iced::advanced::widget::operation::map(
        crate::library_menu::bounds_of(id),
        move |bounds| *sink.lock().unwrap() = bounds,
    )));
    let bounds = found.lock().unwrap().expect("the row");
    (bounds.y, bounds.y + bounds.height)
}

fn shows<M>(laid: &mut Laid<'_, M>, id: &str) -> bool {
    let offset = laid.offsets()[0];
    let (top, bottom) = span(laid, id);
    top >= offset - 0.5 && bottom <= offset + VIEWPORT + 0.5
}

/// Presses `key` and runs the reveal the app runs after it.
fn step_and_reveal(laid: &mut Laid<'_, String>, key: Named, ids: &[String]) {
    press(laid, key, Modifiers::empty());
    run_task(laid, reveal::<String>(ids.to_vec()));
}

#[test]
fn moving_past_the_visible_rows_scrolls_just_enough_and_the_top_scrolls_back() {
    let ids = names("row", 12);
    let mut laid = lay_out(scrolled(&ids, VIEWPORT), Size::new(400.0, VIEWPORT));
    focus(&mut laid, &ids[0]);
    // The second row already shows, so nothing scrolls.
    step_and_reveal(&mut laid, Named::ArrowDown, &ids);
    assert_eq!(laid.offsets(), [0.0]);
    let mut last = 0.0;
    for id in &ids[2..] {
        step_and_reveal(&mut laid, Named::ArrowDown, &ids);
        assert_eq!(laid.focused(), [Id::from(id.clone())]);
        assert!(shows(&mut laid, id), "{id}");
        // Just enough: the row sits at the bottom edge once it scrolled.
        let offset = laid.offsets()[0];
        if offset > last {
            let (_, bottom) = span(&mut laid, id);
            assert!((bottom - (offset + VIEWPORT)).abs() < 0.5, "{id}");
        }
        last = offset;
    }
    assert!(last > 0.0);
    for id in ids.iter().rev().skip(1) {
        step_and_reveal(&mut laid, Named::ArrowUp, &ids);
        assert!(shows(&mut laid, id), "{id}");
    }
    // The first row brings the heading above it back too.
    assert_eq!(laid.offsets(), [0.0]);
}

#[test]
fn a_short_list_never_scrolls() {
    let ids = names("row", 3);
    let mut laid = lay_out(scrolled(&ids, 600.0), Size::new(400.0, 600.0));
    focus(&mut laid, &ids[0]);
    for key in [
        Named::ArrowDown,
        Named::ArrowDown,
        Named::ArrowDown,
        Named::ArrowUp,
        Named::ArrowUp,
    ] {
        step_and_reveal(&mut laid, key, &ids);
        assert_eq!(laid.offsets(), [0.0]);
    }
}

#[test]
fn reveal_does_nothing_without_a_focused_row() {
    let ids = names("row", 12);
    let mut laid = lay_out(scrolled(&ids, VIEWPORT), Size::new(400.0, VIEWPORT));
    laid.run(Box::new(
        iced::advanced::widget::operation::scrollable::snap_to::<()>(
            Id::new(SCROLL_ID),
            iced::widget::scrollable::RelativeOffset {
                x: None,
                y: Some(1.0),
            },
        ),
    ));
    let bottom = laid.offsets()[0];
    assert!(bottom > 0.0);
    run_task(&mut laid, reveal::<String>(ids));
    assert_eq!(laid.offsets(), [bottom]);
}

#[test]
fn a_list_that_opens_scrolled_away_shows_its_first_row() {
    let ids = names("row", 12);
    let mut laid = lay_out(scrolled(&ids, VIEWPORT), Size::new(400.0, VIEWPORT));
    laid.run(Box::new(
        iced::advanced::widget::operation::scrollable::snap_to::<()>(
            Id::new(SCROLL_ID),
            iced::widget::scrollable::RelativeOffset {
                x: None,
                y: Some(1.0),
            },
        ),
    ));
    assert!(laid.offsets()[0] > 0.0);
    let rows = ListRows {
        list: LIST,
        ids: ids.clone(),
    };
    run_task(&mut laid, land::<String>(None, Some(&rows)).unwrap());
    assert_eq!(laid.focused(), [Id::from(ids[0].clone())]);
    assert_eq!(laid.offsets(), [0.0]);
}

#[test]
fn a_row_the_list_moved_out_of_view_is_shown_again() {
    let before = names("row", 12);
    let mut laid = lay_out(scrolled(&before, VIEWPORT), Size::new(400.0, VIEWPORT));
    focus(&mut laid, &before[0]);
    // Rows join above it, so the focused row is pushed below the viewport.
    let after: Vec<String> = names("new", 8).into_iter().chain(before.clone()).collect();
    let mut laid = relay_out(
        scrolled(&after, VIEWPORT),
        laid.into_tree(),
        Size::new(400.0, VIEWPORT),
    );
    assert_eq!(laid.focused(), [Id::from(before[0].clone())]);
    assert!(!shows(&mut laid, &before[0]));
    let task = land::<String>(
        Some(&ListRows {
            list: LIST,
            ids: before.clone(),
        }),
        Some(&ListRows {
            list: LIST,
            ids: after,
        }),
    )
    .expect("a reveal");
    run_task(&mut laid, task);
    assert!(shows(&mut laid, &before[0]));
}
