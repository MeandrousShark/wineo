use std::sync::{Arc, Mutex};

use iced::mouse::{self, Cursor};
use iced::{Color, Font, Point, Rectangle, Size};
use wineo_core::library::{ListColumn, set_list_column};
use wineo_core::paths::Paths;
use wineo_core::runtime::State;

use super::*;
use crate::grid_motion::ViewLayout;
use crate::layout_probe::{Laid, bounds_at, lay_out, layout, relay_out};
use crate::list_view::{
    HEADER_GAP, HEADER_INSET, RowFacts, TITLE_MIN, Widths, columns, row_text, state_room,
    title_cell, title_room,
};
use crate::sort_control::{Field, save};
use crate::tests::card;

fn paths(root: &tempfile::TempDir) -> Paths {
    let home = root.path().to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

/// An app in the list with `count` games, its viewport `width` wide, booted
/// over a `library.toml` that `setup` wrote.
fn listed_with(count: usize, width: f32, setup: impl FnOnce(&Paths)) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(&root);
    setup(&paths);
    let mut app = App::boot(paths).0;
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.layout = ViewLayout::List;
    app.viewport_width = width;
    app.snap_grid();
    (root, app)
}

fn listed(width: f32) -> (tempfile::TempDir, App) {
    listed_with(3, width, |_| {})
}

/// The viewport whose list rows are `slot` wide with the panel closed.
fn viewport(slot: f32) -> f32 {
    slot + 2.0 * crate::grid_motion::PADDING
}

fn send(app: &mut App, event: Event) -> Task<Message> {
    app.update(Message::Columns(event))
}

fn widths(app: &App) -> [f32; 5] {
    ListColumn::ALL.map(|column| app.list_widths.get(column))
}

/// Runs `column`'s write in flight, as its task would, and hands the result
/// back.
fn finish_write(app: &mut App, column: ListColumn) {
    let width = *app
        .view_writes
        .columns
        .get(&column)
        .expect("a column write");
    let written = Field::Column(column, width);
    let result = save(&app.paths, &app.shown, written);
    let _ = app.update(Message::Sort(crate::sort_control::Event::Saved(
        written, result,
    )));
}

fn saved(app: &App) -> std::collections::BTreeMap<ListColumn, u16> {
    wineo_core::library::load(&app.paths).view.list_columns
}

/// The header laid out alone, as wide as the viewport.
fn header(app: &App) -> Laid<'_, Message> {
    lay_out(app.list_header(), Size::new(app.viewport_width, 100.0))
}

/// The header's inner container, which holds the row of cells.
fn inner(laid: &Laid<'_, Message>) -> Rectangle {
    bounds_at(&laid.node, &[0])
}

/// The header cell at `index`: Title's is 0, State's 1, and the others
/// follow in the order they show.
fn cell(laid: &Laid<'_, Message>, index: usize) -> Rectangle {
    bounds_at(&laid.node, &[0, 0, index])
}

/// How many cells the header shows.
fn cell_count(laid: &Laid<'_, Message>) -> usize {
    iced::advanced::Layout::new(&laid.node)
        .children()
        .next()
        .and_then(|inner| inner.children().next())
        .unwrap()
        .children()
        .count()
}

fn cell_index(app: &App, column: ListColumn) -> usize {
    match column {
        ListColumn::State => 1,
        _ => {
            2 + app
                .list_columns()
                .iter()
                .position(|shown| shown.list_column() == column)
                .expect("the column shows")
        }
    }
}

/// The left edge of the gap left of `column`'s header cell.
fn gap(app: &App, column: ListColumn) -> f32 {
    let laid = header(app);
    let left = cell(&laid, cell_index(app, column) - 1);
    left.x + left.width
}

fn press_at(at: Point) -> (iced::Event, Cursor) {
    (
        iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Cursor::Available(at),
    )
}

fn release_at(at: Point) -> (iced::Event, Cursor) {
    (
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        Cursor::Available(at),
    )
}

fn move_to(at: Point) -> (iced::Event, Cursor) {
    (
        iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
        Cursor::Available(at),
    )
}

fn redraw() -> iced::Event {
    iced::Event::Window(iced::window::Event::RedrawRequested(
        std::time::Instant::now(),
    ))
}

fn events(laid: &mut Laid<'_, Message>, events: &[(iced::Event, Cursor)]) -> Vec<Message> {
    events
        .iter()
        .flat_map(|(event, cursor)| laid.event_at(event, *cursor))
        .collect()
}

/// What `messages` say, leaving out the cursor place the window notes on
/// every press for the action menu.
fn debug(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| !matches!(message, Message::MenuAnchor(_)))
        .map(|message| format!("{message:?}"))
        .collect()
}

/// The RGBA pixel at `x`, `y` of a `width` wide frame.
fn pixel(pixels: &[u8], width: f32, x: f32, y: f32) -> [u8; 4] {
    let at = (y as usize * width as usize + x as usize) * 4;
    pixels[at..at + 4].try_into().unwrap()
}

fn near(a: [u8; 4], b: Color) -> bool {
    a.iter()
        .zip(b.into_rgba8())
        .all(|(a, b)| a.abs_diff(b) <= 2)
}

/// The x of every pixel that differs between two frames `width` wide.
fn changed_columns(a: &[u8], b: &[u8], width: f32) -> std::collections::BTreeSet<usize> {
    a.chunks(4)
        .zip(b.chunks(4))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| index % width as usize)
        .collect()
}

#[test]
fn custom_widths_change_which_columns_fit_the_title_room_and_the_state_room() {
    use Column::*;
    let mut narrow = Widths::default();
    narrow.set(ListColumn::State, 96.0);
    narrow.set(ListColumn::Tags, 104.0);
    // The defaults need 888 for all four; these are 32 and 72 narrower.
    assert_eq!(columns(783.0, &narrow), &[Playtime, LastPlayed, Source]);
    assert_eq!(columns(784.0, &narrow), &Column::ALL[..]);
    assert_eq!(
        columns(784.0, &Widths::default()),
        &[Playtime, LastPlayed, Source]
    );
    let mut wide = Widths::default();
    wide.set(ListColumn::Playtime, 300.0);
    // 360 for Title and State, then 316 for Playtime.
    assert_eq!(columns(675.0, &wide), &[][..]);
    assert_eq!(columns(676.0, &wide), &[Playtime]);

    let (_root, mut app) = listed(1400.0);
    let mut long = card("long", State::SteamRunning, 0);
    long.tags = vec!["rpg".into(), "open-world".into(), "fantasy".into()];
    let _ = app.update(Message::Loaded(Ok(vec![long])));
    let facts: RowFacts = {
        let card = &app.cards[0];
        app.row_facts(&card.name.as_str().into(), card, app.palette)
    };
    let all = &Column::ALL[..];
    // Title gets what the narrower columns gave up.
    assert_eq!(
        title_room(&facts, all, 1400.0, &narrow)
            - title_room(&facts, all, 1400.0, &Widths::default()),
        32.0 + 72.0
    );
    assert_eq!(state_room(96.0), 96.0 - 16.0);
    let mut tiny = narrow;
    tiny.set(ListColumn::State, 56.0);
    tiny.set(ListColumn::Tags, 56.0);
    let text = row_text(&facts, all, 1400.0, &tiny);
    let measure = |text: &str| crate::tile_title::line_width(text, 13.0, Font::DEFAULT);
    assert!(text.state.ends_with("..."), "{}", text.state);
    assert!(measure(&text.state) <= state_room(56.0));
    let tags = &text
        .cells
        .iter()
        .find(|(column, _)| *column == Tags)
        .unwrap()
        .1;
    assert!(tags.ends_with("..."), "{tags}");
    assert!(measure(tags) <= 56.0);
    // At the defaults the same state and tags keep more.
    let roomy = row_text(&facts, all, 1400.0, &Widths::default());
    let roomy_tags = &roomy
        .cells
        .iter()
        .find(|(column, _)| *column == Tags)
        .unwrap()
        .1;
    assert!(roomy_tags.len() > tags.len(), "{roomy_tags} vs {tags}");
    assert!(roomy.state.len() > text.state.len(), "{}", roomy.state);
}

#[test]
fn the_row_and_header_lay_out_with_the_widths_in_use() {
    let (_root, mut app) = listed(1400.0);
    app.list_widths.set(ListColumn::State, 90.0);
    app.list_widths.set(ListColumn::Source, 150.0);
    let laid = header(&app);
    assert_eq!(cell(&laid, 1).width, 90.0 + 2.0 * HEADER_INSET);
    assert_eq!(cell(&laid, 4).width, 150.0 + 2.0 * HEADER_INSET);
    let card = app.cards[0].clone();
    let width = app.slot_width();
    let node = layout(
        &mut app.row_drawn(
            &card.name.as_str().into(),
            &card,
            1.0,
            width,
            app.list_columns(),
        ),
        Size::new(2000.0, 500.0),
    );
    // The row's container, then its cells.
    assert_eq!(bounds_at(&node, &[0, 1]).width, 90.0);
    assert_eq!(bounds_at(&node, &[0, 4]).width, 150.0);
}

#[test]
fn widths_load_at_start() {
    let (_root, app) = listed_with(1, 1400.0, |paths| {
        set_list_column(paths, ListColumn::State, Some(96)).unwrap();
        set_list_column(paths, ListColumn::Tags, Some(104)).unwrap();
    });
    assert_eq!(widths(&app), [96.0, 96.0, 112.0, 80.0, 104.0]);
}

#[test]
fn a_hand_edit_made_while_the_window_is_open_isnt_picked_up() {
    let (_root, mut app) = listed(1400.0);
    set_list_column(&app.paths, ListColumn::Tags, Some(104)).unwrap();
    let task = app.update(Message::Refresh);
    let messages = crate::sort_control::tests::answers(&app, task);
    assert!(!messages.is_empty(), "the refresh read the library");
    for message in messages {
        let _ = app.update(message);
    }
    assert_eq!(widths(&app), [128.0, 96.0, 112.0, 80.0, 176.0]);
    // The file keeps the edit, which the next start reads.
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&104));
}

#[test]
fn a_drag_changes_one_width_and_keeps_the_divider_under_the_pointer() {
    let (_root, mut app) = listed(1400.0);
    let (tags, source, title) = {
        let before = header(&app);
        (cell(&before, 5), cell(&before, 4), cell(&before, 0))
    };
    let x = gap(&app, ListColumn::Tags) + 2.0;
    let _ = send(&mut app, Event::Press(ListColumn::Tags, x));
    let _ = send(&mut app, Event::Move(x + 40.0));
    assert_eq!(widths(&app), [128.0, 96.0, 112.0, 80.0, 136.0]);
    {
        let after = header(&app);
        // Tags' right edge stays, everything left of it slides with the
        // pointer, and Title takes up the change.
        assert_eq!(gap(&app, ListColumn::Tags), x + 40.0 - 2.0);
        assert_eq!(
            cell(&after, 5).x + cell(&after, 5).width,
            tags.x + tags.width
        );
        assert_eq!(cell(&after, 4).x, source.x + 40.0);
        assert_eq!(cell(&after, 0).width, title.width + 40.0);
    }
    let _ = send(&mut app, Event::Move(x - 30.0));
    assert_eq!(widths(&app), [128.0, 96.0, 112.0, 80.0, 206.0]);
    // Whole pixels only.
    let _ = send(&mut app, Event::Move(x - 30.4));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 206.0);
    let _ = send(&mut app, Event::Release);
    assert!(app.column_drag.is_none());
    assert_eq!(widths(&app), [128.0, 96.0, 112.0, 80.0, 206.0]);
    // Each handle moves its own column only.
    for column in ListColumn::ALL {
        let (_root, mut app) = listed(1400.0);
        let x = gap(&app, column) + 2.0;
        let _ = send(&mut app, Event::Press(column, x));
        let _ = send(&mut app, Event::Move(x + 20.0));
        let _ = send(&mut app, Event::Release);
        for other in ListColumn::ALL {
            let moved = if other == column { 20.0 } else { 0.0 };
            assert_eq!(
                app.list_widths.get(other),
                f32::from(other.default_width()) - moved,
                "{column:?} moved {other:?}"
            );
        }
    }
}

#[test]
fn a_drag_cant_widen_past_titles_minimum_with_every_column_showing_or_some_dropped() {
    // 888 fits every column with Title at exactly its minimum.
    let (_root, mut app) = listed(viewport(888.0));
    assert_eq!(app.list_columns(), &Column::ALL[..]);
    let _ = send(&mut app, Event::Press(ListColumn::Playtime, 500.0));
    let _ = send(&mut app, Event::Move(400.0));
    assert_eq!(app.list_widths.get(ListColumn::Playtime), 96.0);
    let _ = send(&mut app, Event::Move(530.0));
    assert_eq!(app.list_widths.get(ListColumn::Playtime), 66.0);
    // Back toward the start, but no further.
    let _ = send(&mut app, Event::Move(300.0));
    assert_eq!(app.list_widths.get(ListColumn::Playtime), 96.0);
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.list_columns(), &Column::ALL[..]);

    // 700 drops Tags and leaves Title 4 over its minimum.
    let (_root, mut app) = listed(viewport(700.0));
    assert_eq!(app.list_columns(), &Column::ALL[..3]);
    let _ = send(&mut app, Event::Press(ListColumn::Source, 600.0));
    let _ = send(&mut app, Event::Move(500.0));
    assert_eq!(app.list_widths.get(ListColumn::Source), 84.0);
    let width = app.slot_width();
    assert_eq!(
        title_cell(&app.list_widths, app.list_columns(), width),
        TITLE_MIN
    );
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.list_columns(), &Column::ALL[..3]);
}

#[test]
fn a_drag_that_starts_with_title_under_its_minimum_doesnt_jump_and_can_still_narrow() {
    // State never leaves, so a wide State in a narrow list squeezes Title.
    let (_root, mut app) = listed_with(3, viewport(400.0), |paths| {
        set_list_column(paths, ListColumn::State, Some(300)).unwrap();
    });
    let width = app.slot_width();
    assert!(title_cell(&app.list_widths, app.list_columns(), width) < TITLE_MIN);
    let _ = send(&mut app, Event::Press(ListColumn::State, 200.0));
    let _ = send(&mut app, Event::Move(200.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 300.0);
    let _ = send(&mut app, Event::Move(190.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 300.0);
    let _ = send(&mut app, Event::Move(250.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 250.0);
    let _ = send(&mut app, Event::Move(220.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 280.0);
    let _ = send(&mut app, Event::Move(100.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 300.0);
    let _ = send(&mut app, Event::Move(900.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 56.0);

    // Squeezed so far that Title's minimum would leave State under its own,
    // where a clamp between the two would panic.
    let (_root, mut app) = listed_with(3, viewport(250.0), |paths| {
        set_list_column(paths, ListColumn::State, Some(300)).unwrap();
    });
    let _ = send(&mut app, Event::Press(ListColumn::State, 200.0));
    let _ = send(&mut app, Event::Move(150.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 300.0);
    let _ = send(&mut app, Event::Move(300.0));
    assert_eq!(app.list_widths.get(ListColumn::State), 200.0);
    let _ = send(&mut app, Event::Release);
    let _ = send(&mut app, Event::Reset(ListColumn::State));
    assert_eq!(app.list_widths.get(ListColumn::State), 128.0);
}

#[test]
fn narrowing_with_columns_dropped_holds_the_set_until_release_then_brings_one_back() {
    // 860 drops Tags, which needs 888.
    let (_root, mut app) = listed(viewport(860.0));
    let three = &Column::ALL[..3];
    assert_eq!(app.list_columns(), three);
    let x = gap(&app, ListColumn::LastPlayed) + 2.0;
    let _ = send(&mut app, Event::Press(ListColumn::LastPlayed, x));
    let _ = send(&mut app, Event::Move(x + 56.0));
    assert_eq!(app.list_widths.get(ListColumn::LastPlayed), 56.0);
    // Tags would fit now, but nothing moves under the pointer.
    assert_eq!(
        columns(app.slot_width(), &app.list_widths),
        &Column::ALL[..]
    );
    assert_eq!(app.list_columns(), three);
    assert_eq!(cell_count(&header(&app)), 2 + 3);
    assert_eq!(gap(&app, ListColumn::LastPlayed), x + 56.0 - 2.0);
    // So do the rows: the first row's cells.
    let grid = layout(&mut app.grid(), Size::new(app.viewport_width, 2000.0));
    let row = iced::advanced::Layout::new(&grid)
        .children()
        .next()
        .and_then(|row| row.children().next())
        .unwrap();
    assert_eq!(row.children().count(), 2 + 3);
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.list_columns(), &Column::ALL[..]);
    assert_eq!(cell_count(&header(&app)), 2 + 4);
}

#[test]
fn a_row_fading_out_mid_drag_fits_its_own_width_not_the_held_columns() {
    let (_root, mut app) = listed(viewport(860.0));
    let x = gap(&app, ListColumn::LastPlayed) + 2.0;
    let _ = send(&mut app, Event::Press(ListColumn::LastPlayed, x));
    let _ = send(&mut app, Event::Move(x + 56.0));
    assert_eq!(app.list_columns(), &Column::ALL[..3]);
    let _ = app.update(Message::SearchChanged("game-001".into()));
    assert!(!app.grid_motion.leaving().is_empty(), "a row fades out");
    let grid = layout(&mut app.grid(), Size::new(app.viewport_width, 2000.0));
    // Each row is its container, then its cells.
    let mut counts: Vec<usize> = iced::advanced::Layout::new(&grid)
        .children()
        .filter_map(|row| row.children().next())
        .map(|cells| cells.children().count())
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, [2 + 3, 2 + 4, 2 + 4]);
}

#[test]
fn a_column_stays_between_its_minimum_and_maximum() {
    let (_root, mut app) = listed(2400.0);
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(2000.0));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 56.0);
    let _ = send(&mut app, Event::Move(0.0));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 480.0);
    let _ = send(&mut app, Event::Release);
    assert_eq!(saved(&app).get(&ListColumn::Tags), None, "not written yet");
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&480));
}

#[test]
fn a_double_click_resets_to_the_default_or_as_near_as_title_allows() {
    // Where the default fits.
    let (_root, mut app) = listed_with(3, 1400.0, |paths| {
        set_list_column(paths, ListColumn::Tags, Some(100)).unwrap();
    });
    let _ = send(&mut app, Event::Reset(ListColumn::Tags));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 176.0);
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), None);
    // A narrower default goes all the way back too.
    let (_root, mut app) = listed_with(3, viewport(900.0), |paths| {
        set_list_column(paths, ListColumn::Source, Some(300)).unwrap();
    });
    let _ = send(&mut app, Event::Reset(ListColumn::Source));
    assert_eq!(app.list_widths.get(ListColumn::Source), 80.0);

    // Where Title's minimum stops it short: Tags at 56 in a list 30 wider
    // than every column needs.
    let (_root, mut app) = listed_with(3, viewport(888.0 - 120.0 + 30.0), |paths| {
        set_list_column(paths, ListColumn::Tags, Some(56)).unwrap();
    });
    assert_eq!(app.list_columns(), &Column::ALL[..]);
    let _ = send(&mut app, Event::Reset(ListColumn::Tags));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 86.0);
    assert_eq!(app.list_columns(), &Column::ALL[..], "no column left");
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&86));
    // With no room at all it stays put and writes nothing.
    let _ = send(&mut app, Event::Reset(ListColumn::Tags));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 86.0);
    assert!(app.view_writes.columns.is_empty());
}

#[test]
fn the_hover_line_sits_in_the_gap_on_its_third_pixel() {
    let (_root, app) = listed(1400.0);
    let laid = header(&app);
    let bounds = inner(&laid);
    assert_eq!(HEADER_GAP, 4.0);
    for column in ListColumn::ALL {
        let index = cell_index(&app, column);
        let left = cell(&laid, index - 1);
        let right = cell(&laid, index);
        let gap = left.x + left.width;
        assert_eq!(right.x - gap, HEADER_GAP, "{column:?}");
        // 8 px centered on the gap, 8 px left of where the label starts.
        let hit = hit_area(bounds, gap);
        assert_eq!((hit.x, hit.width), (gap - 2.0, 8.0), "{column:?}");
        assert_eq!(hit.center_x(), right.x + HEADER_INSET - 8.0, "{column:?}");
        assert_eq!(hit.height, crate::list_view::HEADER_HEIGHT);
        // The third of the gap's four pixels, nearer the column it resizes.
        let line = line(bounds, gap);
        assert_eq!((line.x, line.width), (gap + 2.0, 1.0), "{column:?}");
        assert_eq!(line.height, crate::list_view::HEADER_HEIGHT);
    }
}

/// The middle pixel of every handle's line in `frame`, in `ListColumn::ALL`
/// order.
fn middles(app: &App, frame: &[u8], bounds: Rectangle) -> Vec<[u8; 4]> {
    ListColumn::ALL
        .into_iter()
        .map(|column| {
            let x = gap(app, column) + LINE_OFFSET;
            pixel(frame, app.viewport_width, x, bounds.center_y())
        })
        .collect()
}

/// What the muted color looks like at the strength the lines draw it, as
/// the frame holds it: the screenshot's color is premultiplied by alpha.
fn faint(app: &App) -> Color {
    let muted = app.palette.muted;
    Color::from_rgba(
        muted.r * MUTED_STRENGTH,
        muted.g * MUTED_STRENGTH,
        muted.b * MUTED_STRENGTH,
        MUTED_STRENGTH,
    )
}

#[test]
fn nothing_shows_at_rest_every_line_muted_on_the_header_and_a_primary_one_while_dragging() {
    let (_root, mut app) = listed(1400.0);
    let width = app.viewport_width;
    let mut laid = header(&app);
    let bounds = inner(&laid);
    let _ = laid.event_at(&redraw(), Cursor::Unavailable);
    let rest = laid.draw_at(Cursor::Unavailable);
    for column in ListColumn::ALL {
        let x = gap(&app, column) + LINE_OFFSET;
        for y in bounds.y as usize..(bounds.y + bounds.height) as usize {
            assert_eq!(pixel(&rest, width, x, y as f32)[3], 0, "{column:?} at {y}");
        }
    }
    let every_line: std::collections::BTreeSet<usize> = ListColumn::ALL
        .into_iter()
        .map(|column| (gap(&app, column) + LINE_OFFSET) as usize)
        .collect();
    for column in ListColumn::ALL {
        let gap = gap(&app, column);
        // Anywhere in the hit area, even over a button's edge, every line
        // shows the same, the hovered one included.
        for dx in [-2.0, 0.5, 3.0, 5.5] {
            let at = Cursor::Available(Point::new(gap + dx, bounds.center_y()));
            let _ = laid.event_at(&redraw(), at);
            let hovered = laid.draw_at(at);
            assert_eq!(
                changed_columns(&rest, &hovered, width),
                every_line,
                "{column:?} at {dx}"
            );
            for middle in middles(&app, &hovered, bounds) {
                assert!(near(middle, faint(&app)), "{column:?}: {middle:?}");
            }
            assert_eq!(
                laid.interaction_at(at),
                mouse::Interaction::ResizingHorizontally
            );
        }
        // Just outside it, the lines stay and the cursor isn't the resize one.
        let outside = Cursor::Available(Point::new(gap + 6.5, bounds.center_y()));
        let _ = laid.event_at(&redraw(), outside);
        assert_ne!(
            laid.interaction_at(outside),
            mouse::Interaction::ResizingHorizontally
        );
    }

    drop(laid);
    let gap = gap(&app, ListColumn::Source);
    let _ = send(&mut app, Event::Press(ListColumn::Source, gap + 2.0));
    let mut laid = header(&app);
    let _ = laid.event_at(&redraw(), Cursor::Unavailable);
    let dragging = laid.draw_at(Cursor::Unavailable);
    assert_eq!(changed_columns(&rest, &dragging, width), every_line);
    for (column, middle) in ListColumn::ALL
        .into_iter()
        .zip(middles(&app, &dragging, bounds))
    {
        let wanted = if column == ListColumn::Source {
            app.palette.primary
        } else {
            faint(&app)
        };
        assert!(near(middle, wanted), "{column:?}: {middle:?}");
    }
    // The header's height only.
    let below = pixel(
        &dragging,
        width,
        gap + LINE_OFFSET,
        bounds.y + bounds.height,
    );
    assert_eq!(below[3], 0);
    // The resize cursor stays wherever the pointer goes during a drag.
    let mut laid = header(&app);
    let _ = events(
        &mut laid,
        &[press_at(Point::new(gap + 2.0, bounds.center_y()))],
    );
    let away = Cursor::Available(Point::new(300.0, 90.0));
    assert_eq!(
        laid.interaction_at(away),
        mouse::Interaction::ResizingHorizontally
    );
}

#[test]
fn a_pointer_on_a_label_shows_every_line_and_off_the_header_none() {
    let (_root, app) = listed(1400.0);
    let mut laid = header(&app);
    let bounds = inner(&laid);
    let playtime = cell(&laid, 2);
    let on_label = Cursor::Available(playtime.center());
    let _ = laid.event_at(&redraw(), Cursor::Unavailable);
    let rest = laid.draw_at(Cursor::Unavailable);
    let _ = laid.event_at(&redraw(), on_label);
    let labelled = laid.draw_at(on_label);
    // The label's own hover look changes pixels too, so look at the lines.
    for (column, middle) in ListColumn::ALL
        .into_iter()
        .zip(middles(&app, &labelled, bounds))
    {
        assert!(near(middle, faint(&app)), "{column:?}: {middle:?}");
    }
    assert_ne!(
        laid.interaction_at(on_label),
        mouse::Interaction::ResizingHorizontally
    );
    // Below the header, where nothing of it is.
    let off = Cursor::Available(Point::new(
        playtime.center().x,
        bounds.y + bounds.height + 20.0,
    ));
    let _ = laid.event_at(&redraw(), off);
    assert_eq!(laid.draw_at(off), rest);
    for middle in middles(&app, &laid.draw_at(off), bounds) {
        assert_eq!(middle[3], 0);
    }
}

#[test]
fn a_drag_with_the_pointer_off_the_header_keeps_the_dragged_line_primary_and_the_rest_muted() {
    let (_root, mut app) = listed(1400.0);
    let gap = gap(&app, ListColumn::Tags);
    let _ = send(&mut app, Event::Press(ListColumn::Tags, gap + 2.0));
    let mut laid = header(&app);
    let bounds = inner(&laid);
    let off = Cursor::Available(Point::new(300.0, bounds.y + bounds.height + 40.0));
    assert!(!off.is_over(bounds));
    let _ = laid.event_at(&redraw(), off);
    let frame = laid.draw_at(off);
    let drawn = middles(&app, &frame, bounds);
    for (column, middle) in ListColumn::ALL.into_iter().zip(drawn) {
        let wanted = if column == ListColumn::Tags {
            app.palette.primary
        } else {
            faint(&app)
        };
        assert!(near(middle, wanted), "{column:?}: {middle:?}");
    }
}

#[test]
fn a_line_fades_in_from_each_end_and_the_muted_one_is_fainter_than_the_primary() {
    let (_root, mut app) = listed(1400.0);
    let width = app.viewport_width;
    let source = gap(&app, ListColumn::Source) + LINE_OFFSET;
    let tags = gap(&app, ListColumn::Tags) + LINE_OFFSET;
    let _ = send(&mut app, Event::Press(ListColumn::Source, source));
    let mut laid = header(&app);
    let bounds = inner(&laid);
    let off = Cursor::Available(Point::new(300.0, bounds.y + bounds.height + 40.0));
    let _ = laid.event_at(&redraw(), off);
    let frame = laid.draw_at(off);
    let alpha_at = |x: f32, y: f32| pixel(&frame, width, x, y)[3];
    let top = bounds.y;
    let quarter = bounds.height * FADE;
    for x in [source, tags] {
        let middle = alpha_at(x, bounds.center_y());
        // The ends are fainter than the middle, and the fade rises toward it.
        for y in [top, top + quarter / 2.0, bounds.y + bounds.height - 1.0] {
            assert!(alpha_at(x, y) < middle, "x {x} y {y}");
        }
        assert!(alpha_at(x, top) < alpha_at(x, top + quarter / 2.0));
        // Solid between the fades.
        assert_eq!(alpha_at(x, top + quarter + 1.0), middle);
        assert_eq!(alpha_at(x, top + bounds.height - quarter - 2.0), middle);
    }
    let primary = alpha_at(source, bounds.center_y());
    let muted = alpha_at(tags, bounds.center_y());
    assert!(primary >= 253, "{primary}");
    assert!(muted < primary && muted > 0, "{muted}");
    assert!(near(
        pixel(&frame, width, tags, bounds.center_y()),
        faint(&app)
    ));
}

#[test]
fn the_sort_buttons_show_no_hover_under_a_handle_or_during_a_drag() {
    let (_root, app) = listed(1400.0);
    let width = app.viewport_width;
    let mut laid = header(&app);
    let bounds = inner(&laid);
    let playtime = cell(&laid, 2);
    let gap = gap(&app, ListColumn::Playtime);
    // The handle's right half lies over Playtime's button.
    let handle_point = Point::new(gap + 5.0, bounds.center_y());
    assert!(playtime.contains(handle_point));
    let on_handle = Cursor::Available(handle_point);
    let on_label = Cursor::Available(playtime.center());
    let frame = |laid: &mut Laid<'_, Message>, cursor: Cursor| {
        if let Some(at) = cursor.position() {
            let _ = laid.event_at(&move_to(at).0, cursor);
        }
        let _ = laid.event_at(&redraw(), cursor);
        laid.draw_at(cursor)
    };
    let rest = frame(&mut laid, Cursor::Unavailable);
    // Changes in Playtime's cell, right of the line's pixel.
    let in_playtime = |changed: std::collections::BTreeSet<usize>| {
        changed
            .into_iter()
            .filter(|x| *x as f32 > gap + LINE_OFFSET && (*x as f32) < playtime.x + playtime.width)
            .count()
    };
    // The check finds a hover where there is one.
    let hovered = frame(&mut laid, on_label);
    assert!(in_playtime(changed_columns(&rest, &hovered, width)) > 0);
    let handle = frame(&mut laid, on_handle);
    assert_eq!(in_playtime(changed_columns(&rest, &handle, width)), 0);
    // During a drag, a label under the pointer stays as it was.
    let _ = events(
        &mut laid,
        &[press_at(Point::new(gap + 2.0, bounds.center_y()))],
    );
    let dragged = frame(&mut laid, on_label);
    assert_eq!(in_playtime(changed_columns(&rest, &dragged, width)), 0);
}

/// Finds the header's place in the whole window.
fn header_in(laid: &mut Laid<'_, Message>) -> Rectangle {
    struct Find(Arc<Mutex<Option<Rectangle>>>);
    impl iced::advanced::widget::Operation for Find {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn container(&mut self, id: Option<&iced::widget::Id>, bounds: Rectangle) {
            if id == Some(&iced::widget::Id::new(HEADER_ID)) {
                *self.0.lock().unwrap() = Some(bounds);
            }
        }
    }
    let found = Arc::new(Mutex::new(None));
    laid.run(Box::new(Find(found.clone())));
    found.lock().unwrap().expect("the list header")
}

#[test]
fn a_click_drag_or_double_click_on_a_handle_doesnt_sort_or_close_the_panel() {
    let (_root, mut app) = listed(1400.0);
    let _ = app.update(Message::Select("game-000".into()));
    assert!(app.panel_open);
    let sort = (app.query.sort, app.query.descending);
    let (offset, gap) = {
        let alone = header(&app);
        let state = cell(&alone, 1);
        (bounds_at(&alone.node, &[]), state.x + state.width)
    };
    let placed = header_in(&mut lay_out(app.view(), app.window_size));
    let at = Point::new(placed.x - offset.x + gap + 2.0, placed.center_y());
    // Each gesture starts on a freshly laid out window, as after an update.
    let gesture = |app: &App, steps: &[(iced::Event, Cursor)]| {
        events(&mut lay_out(app.view(), app.window_size), steps)
    };
    let check = |app: &mut App, messages: Vec<Message>| {
        let text = debug(&messages);
        assert!(
            text.iter()
                .all(|message| !message.starts_with("Sort(") && message != "ClosePanel"),
            "{text:?}"
        );
        for message in messages {
            let _ = app.update(message);
        }
        assert!(app.panel_open);
        assert_eq!((app.query.sort, app.query.descending), sort);
        text
    };

    // A click.
    let messages = gesture(&app, &[press_at(at), release_at(at)]);
    let text = check(&mut app, messages);
    assert_eq!(
        text,
        [
            format!("Columns(Press(Playtime, {:?}))", at.x),
            "Columns(Release)".to_owned()
        ]
    );
    // A drag, which keeps going off the header.
    let off = Point::new(at.x + 30.0, at.y + 300.0);
    let messages = gesture(&app, &[press_at(at), move_to(off), release_at(off)]);
    let text = check(&mut app, messages);
    assert!(
        text.contains(&format!("Columns(Move({:?}))", off.x)),
        "{text:?}"
    );
    assert_eq!(app.list_widths.get(ListColumn::Playtime), 66.0);
    // A double-click on the handle, which followed the divider.
    let moved = Point::new(off.x, at.y);
    let messages = gesture(
        &app,
        &[
            press_at(moved),
            release_at(moved),
            press_at(moved),
            release_at(moved),
        ],
    );
    let text = check(&mut app, messages);
    assert!(
        text.contains(&"Columns(Reset(Playtime))".to_owned()),
        "{text:?}"
    );
    assert_eq!(app.list_widths.get(ListColumn::Playtime), 96.0);
    // A label beside it still sorts.
    let label = Point::new(at.x + 20.0, at.y);
    let text = debug(&gesture(&app, &[press_at(label), release_at(label)]));
    assert_eq!(text, ["Sort(Header(Playtime))"]);
}

#[test]
fn a_drag_saves_once_on_release_and_a_write_finishing_mid_drag_starts_none() {
    let (_root, mut app) = listed(1400.0);
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    for x in [1010.0, 1025.0, 1040.0] {
        let _ = send(&mut app, Event::Move(x));
        assert!(app.view_writes.columns.is_empty());
    }
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 136)].into());
    // A second release, or a click that moves nothing, saves nothing more.
    let _ = send(&mut app, Event::Release);
    let _ = send(&mut app, Event::Press(ListColumn::Source, 900.0));
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 136)].into());
    // The write lands while the same column is dragged again: nothing
    // follows for the widths it passes through.
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(1020.0));
    finish_write(&mut app, ListColumn::Tags);
    assert!(app.view_writes.columns.is_empty());
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&136));
    let _ = send(&mut app, Event::Move(1030.0));
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 106)].into());
    // A release while the earlier write still runs saves after it.
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(1010.0));
    let _ = send(&mut app, Event::Release);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 106)].into());
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 96)].into());
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&96));
    // Another column's write landing mid-drag follows up as usual.
    let _ = send(&mut app, Event::Press(ListColumn::Source, 900.0));
    let _ = send(&mut app, Event::Move(910.0));
    let _ = send(&mut app, Event::Release);
    let _ = send(&mut app, Event::Reset(ListColumn::Source));
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(1010.0));
    finish_write(&mut app, ListColumn::Source);
    assert_eq!(app.view_writes.columns, [(ListColumn::Source, 80)].into());
}

#[test]
fn an_interrupted_drag_saves_where_it_was() {
    fn dragged() -> (tempfile::TempDir, App) {
        let (root, mut app) = listed(1400.0);
        let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
        let _ = send(&mut app, Event::Move(1050.0));
        (root, app)
    }
    let ended = |app: &mut App, case: &str| {
        assert!(app.column_drag.is_none(), "{case}");
        assert_eq!(
            app.view_writes.columns,
            [(ListColumn::Tags, 126)].into(),
            "{case}"
        );
        finish_write(app, ListColumn::Tags);
        assert_eq!(saved(app).get(&ListColumn::Tags), Some(&126), "{case}");
    };
    // The window closing, which waits for the write.
    let (_root, mut app) = dragged();
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.closing && !app.exiting);
    ended(&mut app, "close");
    // Hiding to the tray.
    let (_root, mut app) = dragged();
    let _ = app.hide_window();
    ended(&mut app, "tray");
    // The grid or coverflow.
    for layout in [ViewLayout::Grid, ViewLayout::Coverflow] {
        let (_root, mut app) = dragged();
        let _ = app.update(Message::Layout(crate::layout_switch::Event::Set(layout)));
        assert_eq!(app.layout, layout);
        ended(&mut app, "layout");
    }
    // Settings over the Library, and leaving it.
    let (_root, mut app) = dragged();
    let _ = app.update(Message::Select("game-000".into()));
    let _ = app.update(Message::OpenSettings);
    assert!(app.settings.is_some());
    ended(&mut app, "settings");
    let (_root, mut app) = dragged();
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    ended(&mut app, "section");
    // The stop dialog, whose scrim covers the header but not the panel. It
    // opens in an update like any other, which this move stands in for.
    let (_root, mut app) = dragged();
    app.hook_stop_confirm = Some("game-000".into());
    let _ = send(&mut app, Event::Move(1050.0));
    ended(&mut app, "stop dialog");
}

#[test]
fn a_header_label_and_its_arrow_at_the_minimum_width_stay_inside_the_cell() {
    let (_root, mut app) = listed(1400.0);
    let size = Size::new(app.viewport_width, 100.0);
    for column in Column::ALL {
        app.list_widths = Widths::default();
        app.list_widths.set(column.list_column(), 56.0);
        let _ = app.update(Message::Sort(crate::sort_control::Event::Pick(
            column.sort().unwrap(),
        )));
        let probe = crate::layout_probe::probe(&mut app.list_header(), size);
        let outside = probe.outside(Rectangle::with_size(size));
        assert!(outside.is_empty(), "{column:?}: {outside:?}");
        let index = cell_index(&app, column.list_column());
        let cell = bounds_at(&probe.node, &[0, 0, index]);
        let room = (cell.x + HEADER_INSET, cell.x + cell.width - HEADER_INSET);
        let label = bounds_at(&probe.node, &[0, 0, index, 0, 0, 0]);
        let arrow = bounds_at(&probe.node, &[0, 0, index, 0, 0, 1]);
        assert_eq!(arrow.width, 12.0, "{column:?}: the arrow keeps its room");
        assert!(
            label.x >= room.0 && arrow.x + arrow.width <= room.1,
            "{column:?}"
        );
        assert!(label.x + label.width <= arrow.x, "{column:?}");
    }
    // State, which doesn't sort, is cut to its own width.
    app.list_widths = Widths::default();
    app.list_widths.set(ListColumn::State, 56.0);
    let probe = crate::layout_probe::probe(&mut app.list_header(), size);
    assert!(probe.outside(Rectangle::with_size(size)).is_empty());
    let cell = bounds_at(&probe.node, &[0, 0, 1]);
    let label = bounds_at(&probe.node, &[0, 0, 1, 0]);
    assert_eq!(cell.width, 56.0 + 2.0 * HEADER_INSET);
    assert!(label.width <= 56.0, "{label:?}");
    assert!(
        label.x >= cell.x + HEADER_INSET
            && label.x + label.width <= cell.x + cell.width - HEADER_INSET
    );
}

/// Runs `column`'s write in flight, then the follow-up it starts, if any.
fn finish_writes(app: &mut App, column: ListColumn) {
    while app.view_writes.columns.contains_key(&column) {
        finish_write(app, column);
    }
}

#[test]
fn a_write_landing_mid_drag_is_still_saved_when_the_drag_ends_where_it_started() {
    let (_root, mut app) = listed(1400.0);
    let drag = |app: &mut App, by: f32| {
        let _ = send(app, Event::Press(ListColumn::Tags, 1000.0));
        let _ = send(app, Event::Move(1000.0 + by));
        let _ = send(app, Event::Release);
    };
    drag(&mut app, 10.0);
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 166)].into());
    // Released at 156 while 166 is still being written: 156 is owed.
    drag(&mut app, 10.0);
    assert_eq!(app.list_widths.get(ListColumn::Tags), 156.0);
    // 166 lands during the next drag, which lets go without moving.
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    finish_write(&mut app, ListColumn::Tags);
    assert!(app.view_writes.columns.is_empty());
    let _ = send(&mut app, Event::Release);
    finish_writes(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&156));
}

#[test]
fn a_write_landing_mid_drag_is_still_saved_when_a_double_click_resets_to_the_same_width() {
    let (_root, mut app) = listed(1400.0);
    let drag = |app: &mut App, by: f32| {
        let _ = send(app, Event::Press(ListColumn::Tags, 1000.0));
        let _ = send(app, Event::Move(1000.0 + by));
        let _ = send(app, Event::Release);
    };
    drag(&mut app, 10.0);
    // Back to the default while 166 is still being written.
    drag(&mut app, -10.0);
    assert_eq!(app.list_widths.get(ListColumn::Tags), 176.0);
    // A double-click's first press starts a drag, 166 lands, then the
    // second press resets to the width it already has.
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    finish_write(&mut app, ListColumn::Tags);
    let _ = send(&mut app, Event::Reset(ListColumn::Tags));
    finish_writes(&mut app, ListColumn::Tags);
    assert_eq!(
        saved(&app).get(&ListColumn::Tags),
        None,
        "back at the default"
    );
}

#[test]
fn a_header_made_anew_mid_drag_still_sends_its_moves_and_release() {
    let (_root, mut app) = listed(1400.0);
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    // A fresh header, as when the page's rows above it change mid-drag, and
    // the pointer outside the window.
    let messages = {
        let mut laid = header(&app);
        let away = Point::new(1030.0, 500.0);
        events(
            &mut laid,
            &[
                (move_to(away).0, Cursor::Unavailable),
                (release_at(away).0, Cursor::Unavailable),
            ],
        )
    };
    assert_eq!(
        debug(&messages),
        ["Columns(Move(1030.0))", "Columns(Release)"]
    );
    for message in messages {
        let _ = app.update(message);
    }
    assert!(app.column_drag.is_none());
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 146)].into());
}

#[test]
fn a_drag_with_the_pointer_outside_the_window_still_moves_and_saves() {
    // One header from press to release: this is about the pointer leaving
    // the window, not the header being made anew.
    let (_root, mut app) = listed(1400.0);
    let at = Point::new(gap(&app, ListColumn::Tags) + 2.0, 16.0);
    let mut messages = Vec::new();
    {
        let mut laid = header(&app);
        messages.extend(events(&mut laid, &[press_at(at)]));
        let away = Point::new(at.x + 20.0, -40.0);
        messages.extend(events(
            &mut laid,
            &[
                (move_to(away).0, Cursor::Unavailable),
                (release_at(away).0, Cursor::Unavailable),
            ],
        ));
    }
    assert_eq!(
        debug(&messages),
        [
            format!("Columns(Press(Tags, {:?}))", at.x),
            format!("Columns(Move({:?}))", at.x + 20.0),
            "Columns(Release)".to_owned(),
        ]
    );
    for message in messages {
        let _ = app.update(message);
    }
    finish_write(&mut app, ListColumn::Tags);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&156));
}

/// Whether the header's own state says a drag is going.
fn widget_dragging(tree: &tree::Tree) -> Option<ListColumn> {
    tree.state.downcast_ref::<super::State>().dragging
}

#[test]
fn the_headers_drag_state_stays_while_the_app_drags_and_clears_after() {
    let (_root, mut app) = listed(1400.0);
    let size = Size::new(app.viewport_width, 100.0);
    let at = Point::new(gap(&app, ListColumn::Source) + 2.0, 16.0);
    let (messages, tree) = {
        let mut laid = header(&app);
        let messages = events(&mut laid, &[press_at(at)]);
        (messages, laid.into_tree())
    };
    assert_eq!(widget_dragging(&tree), Some(ListColumn::Source));
    for message in messages {
        let _ = app.update(message);
    }
    let tree = relay_out(app.list_header(), tree, size).into_tree();
    assert_eq!(widget_dragging(&tree), Some(ListColumn::Source), "kept");
    // The app lets go without the header seeing a release, as an
    // interruption does.
    let _ = app.end_column_drag();
    let tree = relay_out(app.list_header(), tree, size).into_tree();
    assert_eq!(widget_dragging(&tree), None, "cleared");
}

#[test]
fn a_handle_captures_its_press_move_and_release_and_a_double_clicks_too() {
    let (_root, app) = listed(1400.0);
    let at = Point::new(gap(&app, ListColumn::Playtime) + 2.0, 16.0);
    let mut laid = header(&app);
    let status = |laid: &mut Laid<'_, Message>, (event, cursor): (iced::Event, Cursor)| {
        laid.event_with_status(&event, cursor).1
    };
    let captured = iced::event::Status::Captured;
    assert_eq!(status(&mut laid, press_at(at)), captured, "press");
    let moved = Point::new(at.x + 5.0, 16.0);
    assert_eq!(status(&mut laid, move_to(moved)), captured, "move");
    assert_eq!(status(&mut laid, release_at(moved)), captured, "release");
    // A fresh header, so the double-click is on one spot.
    let mut laid = header(&app);
    for step in [
        "first press",
        "first release",
        "second press",
        "second release",
    ] {
        let event = if step.ends_with("press") {
            press_at(at)
        } else {
            release_at(at)
        };
        assert_eq!(status(&mut laid, event), captured, "{step}");
    }
    // A release with no handle press before it is left alone.
    let mut laid = header(&app);
    assert_eq!(
        status(&mut laid, release_at(at)),
        iced::event::Status::Ignored
    );
}

#[test]
fn a_narrow_header_has_handles_only_for_the_columns_it_shows() {
    // 600 shows Playtime and Last played, and drops Source and Tags.
    let (_root, app) = listed(viewport(600.0));
    assert_eq!(app.list_columns(), &Column::ALL[..2]);
    let laid = header(&app);
    let bounds = inner(&laid);
    let mut handles: Vec<(f32, f32)> = Vec::new();
    let mut x = bounds.x;
    while x < bounds.x + bounds.width {
        let at = Cursor::Available(Point::new(x, bounds.center_y()));
        if laid.interaction_at(at) == mouse::Interaction::ResizingHorizontally {
            match handles.last_mut() {
                Some((_, end)) if x - *end <= 0.5 => *end = x,
                _ => handles.push((x, x)),
            }
        }
        x += 0.5;
    }
    let gaps: Vec<f32> = [
        ListColumn::State,
        ListColumn::Playtime,
        ListColumn::LastPlayed,
    ]
    .into_iter()
    .map(|column| gap(&app, column))
    .collect();
    assert_eq!(
        handles,
        gaps.iter()
            .map(|gap| (gap - 2.0, gap + 5.5))
            .collect::<Vec<_>>()
    );
    // Nothing past Last played, the last column showing.
    let last = cell(&laid, 3);
    assert!(handles.iter().all(|(_, end)| *end < last.x + last.width));
}

#[test]
fn the_handles_pass_on_their_cells_overlays() {
    use iced::widget::{row, text, tooltip};
    let (_root, app) = listed(1400.0);
    let cells = row![
        tooltip(text("Title"), text("A tip"), tooltip::Position::Bottom),
        text("State"),
    ];
    let content: Element<'_, Message> = iced::widget::container(cells).into();
    let element: Element<'_, Message> =
        Handles::new(content, vec![ListColumn::State], None, app.palette).into();
    let mut laid = lay_out(element, Size::new(400.0, 200.0));
    let title = bounds_at(&laid.node, &[0, 0]);
    let _ = events(&mut laid, &[move_to(title.center())]);
    assert!(!laid.overlay_bounds().is_empty(), "the tooltip shows");
}

#[test]
fn a_write_landing_at_the_width_a_drag_passes_through_is_still_owed() {
    let (_root, mut app) = listed(1400.0);
    let drag = |app: &mut App, by: f32| {
        let _ = send(app, Event::Press(ListColumn::Tags, 1000.0));
        let _ = send(app, Event::Move(1000.0 + by));
        let _ = send(app, Event::Release);
    };
    drag(&mut app, 10.0);
    // Released at 156 while 166 is still being written.
    drag(&mut app, 10.0);
    // A third drag passes back through 166 just as that write lands.
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(990.0));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 166.0);
    finish_write(&mut app, ListColumn::Tags);
    let _ = send(&mut app, Event::Move(1000.0));
    let _ = send(&mut app, Event::Release);
    finish_writes(&mut app, ListColumn::Tags);
    assert_eq!(app.list_widths.get(ListColumn::Tags), 156.0);
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&156));
}

#[test]
fn a_handle_press_whose_release_never_came_doesnt_swallow_a_later_sort_click() {
    let (_root, app) = listed(1400.0);
    let at = Point::new(gap(&app, ListColumn::Playtime) + 2.0, 16.0);
    let label = cell(&header(&app), 2).center();
    // A double-click whose second release went elsewhere.
    let mut laid = header(&app);
    let _ = events(&mut laid, &[press_at(at), release_at(at), press_at(at)]);
    let messages = events(&mut laid, &[press_at(label), release_at(label)]);
    assert_eq!(debug(&messages), ["Sort(Header(Playtime))"]);
    // A little jitter between a double-click's second press and release
    // still leaves the release to the handle.
    let mut laid = header(&app);
    let _ = events(&mut laid, &[press_at(at), release_at(at), press_at(at)]);
    let jitter = Point::new(at.x + 1.0, at.y);
    let _ = events(&mut laid, &[move_to(jitter)]);
    assert_eq!(
        laid.event_with_status(&release_at(jitter).0, Cursor::Available(jitter))
            .1,
        iced::event::Status::Captured
    );
}

#[test]
fn a_drag_the_app_ended_leaves_no_release_owed_to_the_handle() {
    let (_root, mut app) = listed(1400.0);
    let size = Size::new(app.viewport_width, 100.0);
    let at = Point::new(gap(&app, ListColumn::Playtime) + 2.0, 16.0);
    let label = cell(&header(&app), 2).center();
    let (messages, tree) = {
        let mut laid = header(&app);
        (events(&mut laid, &[press_at(at)]), laid.into_tree())
    };
    for message in messages {
        let _ = app.update(message);
    }
    // An interruption, whose scrim then takes the release.
    let _ = app.end_column_drag();
    let mut laid = relay_out(app.list_header(), tree, size);
    assert_eq!(
        laid.event_with_status(&release_at(label).0, Cursor::Available(label))
            .1,
        iced::event::Status::Ignored
    );
}

#[test]
fn losing_focus_ends_a_drag_and_saves_it_once() {
    let (_root, mut app) = listed(1400.0);
    let _ = send(&mut app, Event::Press(ListColumn::Tags, 1000.0));
    let _ = send(&mut app, Event::Move(1050.0));
    let _ = app.update(app.window_event(iced::window::Event::Unfocused));
    assert!(app.column_drag.is_none());
    assert_eq!(app.view_writes.columns, [(ListColumn::Tags, 126)].into());
    // The pointer coming back with no button held moves nothing.
    let _ = send(&mut app, Event::Move(900.0));
    assert_eq!(app.list_widths.get(ListColumn::Tags), 126.0);
    finish_write(&mut app, ListColumn::Tags);
    assert!(app.view_writes.columns.is_empty());
    assert_eq!(saved(&app).get(&ListColumn::Tags), Some(&126));
    // With no drag, losing focus writes nothing.
    let (_root, mut app) = listed(1400.0);
    let before = std::fs::read(app.paths.library_file()).ok();
    let _ = app.update(app.window_event(iced::window::Event::Unfocused));
    assert!(app.view_writes.columns.is_empty());
    assert_eq!(std::fs::read(app.paths.library_file()).ok(), before);
}
