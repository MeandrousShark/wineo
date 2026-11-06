use std::time::{Duration, Instant};

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Font, Rectangle, Size};
use wineo_core::library::SortKey;
use wineo_core::runtime::State;
use wineo_core::source::{Facts, Source};
use wineo_core::steam::AppState;

use super::*;
use crate::grid_motion::{DOCK_WIDTH, ROW_GAP, ViewLayout};
use crate::layout_probe::{bounds_at, click_at, layout};
use crate::sort_control::Event as SortEvent;
use crate::tests::{card, steam_card};

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    (root, App::boot(paths).0)
}

/// An app in the list showing `count` games, in the order of their names.
fn listed(count: usize) -> (tempfile::TempDir, App) {
    let (root, mut app) = fixture();
    let cards = (0..count)
        .map(|index| card(&format!("game-{index:03}"), State::Inactive, 60))
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.layout = ViewLayout::List;
    app.snap_grid();
    (root, app)
}

fn shown(app: &App) -> Vec<String> {
    app.filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect()
}

fn facts(app: &App, name: &str) -> RowFacts {
    let card = app.cards.iter().find(|card| card.name == name).unwrap();
    app.row_facts(card, app.palette)
}

fn answer(app: &mut App, task: iced::Task<Message>) {
    for message in crate::sort_control::tests::answers(app, task) {
        let _ = app.update(message);
    }
}

fn key(app: &mut App, named: Named) {
    let task = app.update(Message::KeyWithFocus(
        Key::Named(named),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    answer(app, task);
}

/// Each shown row's place as the grid widget lays it out.
fn laid_rows(app: &App) -> Vec<Rectangle> {
    let node = layout(
        &mut app.grid(),
        Size::new(app.viewport_width, f32::INFINITY),
    );
    (0..app.filtered().len())
        .map(|index| bounds_at(&node, &[index]))
        .collect()
}

#[test]
fn columns_leave_from_the_right_in_order() {
    use Column::*;
    // Title's 200 and State's 128 need 360 with the padding and gap.
    for (width, expected) in [
        (300.0, &[][..]),
        (471.0, &[][..]),
        (472.0, &[Playtime][..]),
        (599.0, &[Playtime][..]),
        (600.0, &[Playtime, LastPlayed][..]),
        (695.0, &[Playtime, LastPlayed][..]),
        (696.0, &[Playtime, LastPlayed, Source][..]),
        (887.0, &[Playtime, LastPlayed, Source][..]),
        (888.0, &[Playtime, LastPlayed, Source, Tags][..]),
        (2000.0, &Column::ALL[..]),
    ] {
        assert_eq!(columns(width), expected, "{width}");
    }
}

#[test]
fn the_docked_panel_takes_columns_from_the_right_and_the_header_follows() {
    let (_root, mut app) = listed(3);
    app.viewport_width = 1100.0;
    assert_eq!(app.slot_width(), 1100.0 - 32.0);
    assert_eq!(columns(app.slot_width()).len(), 4);
    let _ = app.update(Message::Select("game-000".into()));
    assert!(app.panel_open);
    assert_eq!(app.slot_width(), 1100.0 - DOCK_WIDTH - 32.0);
    assert_eq!(
        columns(app.slot_width()),
        &[Column::Playtime, Column::LastPlayed]
    );
    // The header's labels sit over the same cells as every row's, each
    // header cell reaching its button's inset past them on either side.
    // Both lay out from the same origin, as the scroll's content does.
    let header = layout(&mut app.list_header(), Size::new(1100.0, 100.0));
    let cells = |node: &iced::advanced::layout::Node, path: &[usize]| {
        let mut layout = iced::advanced::Layout::new(node);
        for index in path {
            layout = layout.children().nth(*index).unwrap();
        }
        layout
            .children()
            .map(|cell| (cell.bounds().x, cell.bounds().width))
            .collect::<Vec<_>>()
    };
    let header_cells: Vec<_> = cells(&header, &[0, 0])
        .into_iter()
        .map(|(x, width)| (x + HEADER_INSET, width - 2.0 * HEADER_INSET))
        .collect();
    assert_eq!(header_cells.len(), 2 + 2);
    let grid = layout(
        &mut app.grid(),
        Size::new(app.viewport_width, f32::INFINITY),
    );
    // A row's pointer area takes no layout of its own: its container,
    // then the cells.
    assert_eq!(cells(&grid, &[0, 0]), header_cells);
}

#[test]
fn rows_are_one_full_width_column_where_the_places_say() {
    let (_root, mut app) = listed(6);
    for (width, open) in [(1000.0, false), (1400.0, false), (1400.0, true)] {
        app.viewport_width = width;
        app.panel_open = open;
        app.snap_grid();
        assert_eq!(app.columns(), 1);
        let places = app.grid_places();
        for (index, slot) in places.slots.iter().enumerate() {
            assert_eq!(slot.x, 16.0);
            assert_eq!(slot.y, 16.0 + index as f32 * (ROW_HEIGHT + ROW_GAP));
            assert_eq!(slot.size(), Size::new(app.slot_width(), ROW_HEIGHT));
        }
        assert_eq!(laid_rows(&app), places.slots, "{width}, open {open}");
    }
}

#[test]
fn a_row_shows_what_its_poster_does() {
    let (_root, mut app) = fixture();
    let plain = card("plain", State::Inactive, 0);
    let mut favorite = card("favorite", State::Inactive, 3600);
    favorite.favorite = true;
    let hovered = card("hovered", State::Inactive, 0);
    let mut hidden = card("hidden", State::Inactive, 0);
    hidden.hidden = true;
    let mut missing = steam_card("missing", AppState::NotInstalled);
    missing.source = Source::new(Facts {
        steam: true,
        ..Facts::default()
    });
    let installing = steam_card("installing", AppState::Installing(Some(40)));
    let mut broken = card("broken", State::Inactive, 0);
    broken.error = Some("unreadable".into());
    let running = card("running", State::Running, 0);
    let mut titled = card("hades-goty", State::Inactive, 0);
    titled.display_title = "Hades".into();
    titled.tags = vec!["rpg".into(), "co-op".into()];
    titled.last_played = "yesterday".into();
    titled.source = Source::new(Facts {
        gog_id: Some("1"),
        ..Facts::default()
    });
    let gog = |name: &str, unfinished: bool, install: bool| {
        let mut card = card(name, State::Inactive, 0);
        card.gog = Some(crate::gog_library::GogCard {
            gog_id: Some("1".into()),
            unfinished,
            blocked_by: None,
            install,
            version: None,
        });
        card
    };
    let finish_install = gog("finish-install", true, true);
    let finish_update = gog("finish-update", true, false);
    let update = gog("update", false, false);
    let mut native = card("native", State::Inactive, 0);
    native.source = Source::new(Facts {
        native: true,
        ..Facts::default()
    });
    let _ = app.update(Message::Loaded(Ok(vec![
        plain,
        favorite,
        hovered,
        hidden,
        missing,
        installing,
        broken,
        running,
        titled,
        native,
        finish_install,
        finish_update,
        update,
    ])));
    app.gog_updates.insert(
        "update".to_owned(),
        crate::gog_library::tests::status(
            "update",
            wineo_core::gog::update::UpdateState::Available,
            true,
        ),
    );
    app.layout = ViewLayout::List;
    app.hovered = Some("hovered".into());
    let palette = app.palette;

    let plain = facts(&app, "plain");
    assert_eq!(
        (plain.star, plain.eye, plain.steam, plain.install),
        (Star::None, false, false, None)
    );
    assert_eq!(
        (plain.faded, plain.gray, plain.identifier.clone()),
        (false, false, None)
    );
    assert_eq!((plain.state, plain.dot), ("inactive", palette.muted));
    assert_eq!(plain.playtime, "never played");
    assert_eq!(plain.source, "Windows");

    let favorite = facts(&app, "favorite");
    assert_eq!(favorite.star, Star::Filled);
    assert_eq!(favorite.playtime, "1 h");
    assert_eq!(facts(&app, "hovered").star, Star::Outline);

    let hidden = facts(&app, "hidden");
    assert!(hidden.eye && hidden.faded && !hidden.gray);

    let missing = facts(&app, "missing");
    assert!(missing.steam && missing.faded && missing.gray);
    assert_eq!(missing.state, "not installed");
    assert_eq!(missing.source, "Steam");

    let installing = facts(&app, "installing");
    assert_eq!(installing.install, Some(40));
    assert!(installing.steam && !installing.faded);

    let broken = facts(&app, "broken");
    assert_eq!((broken.state, broken.dot), ("error", palette.warning));
    let running = facts(&app, "running");
    assert_eq!((running.state, running.dot), ("running", palette.success));

    let titled = facts(&app, "hades-goty");
    assert_eq!(titled.title, "Hades");
    assert_eq!(titled.identifier.as_deref(), Some("hades-goty"));
    assert_eq!(titled.tags, "rpg, co-op");
    assert_eq!(titled.last_played, "yesterday");
    assert_eq!(titled.source, "GOG");
    assert_eq!(facts(&app, "native").source, "Native");

    use crate::gog_library::Badge;
    assert_eq!(plain.gog, None);
    for (name, badge, color) in [
        ("finish-install", Badge::FinishInstall, palette.warning),
        ("finish-update", Badge::FinishUpdate, palette.warning),
        ("update", Badge::UpdateAvailable, palette.primary),
    ] {
        let facts = facts(&app, name);
        assert_eq!(facts.gog, Some(badge), "{name}");
        assert_eq!(badge.color(palette), color, "{name}");
        // The pill takes its room from the title, as the marks do.
        assert!(
            title_room(&facts, 1000.0) < title_room(&plain, 1000.0),
            "{name}"
        );
        // And it's drawn: the marks gain it before the star, and the title
        // keeps exactly the room left.
        let card = app.cards.iter().find(|card| card.name == name).unwrap();
        let node = layout(
            &mut app.row_drawn(card, 1.0, 1000.0),
            Size::new(2000.0, 500.0),
        );
        let marks = iced::advanced::Layout::new(&node)
            .children()
            .next()
            .and_then(|cells| cells.children().next())
            .and_then(|title| title.children().nth(2))
            .unwrap();
        assert_eq!(marks.children().count(), 2, "{name}");
        assert_eq!(
            bounds_at(&node, &[0, 0, 1]).width,
            title_room(&facts, 1000.0)
        );
    }

    assert!(!plain.marked && !plain.selected);
    app.toggle_mark("plain");
    app.selected = Some("plain".into());
    let plain = facts(&app, "plain");
    assert!(plain.marked && plain.selected);

    // Every one lays out as one row of the row's height, whatever it shows.
    let width = app.slot_width();
    for card in &app.cards {
        let node = layout(
            &mut app.row_drawn(card, 1.0, width),
            Size::new(2000.0, 500.0),
        );
        assert_eq!(node.size(), Size::new(width, ROW_HEIGHT), "{}", card.name);
    }
}

#[test]
fn a_row_takes_presses_like_a_card() {
    let (_root, app) = listed(3);
    let size = Size::new(app.viewport_width, 2000.0);
    let messages = click_at(app.grid(), size, &[1]);
    assert!(
        messages
            .iter()
            .any(|message| message == "Select(\"game-001\")"),
        "{messages:?}"
    );
    // The star's room on a hovered row favorites it, as the poster's does.
    let (_root, mut app) = listed(3);
    app.hovered = Some("game-001".into());
    let card = app.cards[1].clone();
    let row = app.row_drawn(&card, 1.0, app.slot_width());
    // Cells, title cell, marks, star.
    let messages = click_at(row, Size::new(2000.0, 100.0), &[0, 0, 2, 0]);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("PosterStar(\"game-001\")")),
        "{messages:?}"
    );
}

#[test]
fn ctrl_and_shift_clicks_mark_rows_and_space_and_ctrl_a_do_too() {
    let (_root, mut app) = listed(5);
    let _ = app.update(Message::Select("game-001".into()));
    app.modifiers = Modifiers::CTRL;
    let _ = app.update(Message::Select("game-003".into()));
    app.modifiers = Modifiers::SHIFT;
    let _ = app.update(Message::Select("game-004".into()));
    app.modifiers = Modifiers::empty();
    assert_eq!(
        app.marked.iter().cloned().collect::<Vec<_>>(),
        ["game-003", "game-004"]
    );
    assert_eq!(app.selected.as_deref(), Some("game-001"));
    assert!(facts(&app, "game-003").marked);
    app.space_answered(false);
    assert!(app.marked.contains("game-001"));
    let _ = app.update(Message::KeyWithFocus(
        Key::Character("a".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
        false,
    ));
    assert_eq!(app.marked.len(), 5);
}

#[test]
fn header_clicks_sort_like_the_menu_and_flip_on_a_second_click() {
    for sort in [
        SortKey::Name,
        SortKey::LastPlayed,
        SortKey::Playtime,
        SortKey::Source,
        SortKey::Tags,
    ] {
        let (_root, mut menu) = listed(4);
        let (_root, mut header) = listed(4);
        let _ = menu.update(Message::Sort(SortEvent::Pick(SortKey::Added)));
        let _ = header.update(Message::Sort(SortEvent::Pick(SortKey::Added)));
        let _ = menu.update(Message::Sort(SortEvent::Pick(sort)));
        let _ = header.update(Message::Sort(SortEvent::Header(sort)));
        assert_eq!(header.query, menu.query, "{sort:?}");
        assert_eq!(shown(&header), shown(&menu), "{sort:?}");
        assert_eq!(header.view_writes.sort, menu.view_writes.sort);
        // Again flips it, as the direction button does.
        let _ = menu.update(Message::Sort(SortEvent::FlipDirection));
        let _ = header.update(Message::Sort(SortEvent::Header(sort)));
        assert_eq!(header.query, menu.query, "{sort:?}");
        assert_eq!(header.query.descending, !sort.starts_descending());
        assert_eq!(shown(&header), shown(&menu), "{sort:?}");
        assert_eq!(header.view_writes.descending, menu.view_writes.descending);
    }
}

#[test]
fn each_header_but_state_sends_its_sort() {
    let (_root, mut app) = listed(2);
    app.viewport_width = 1400.0;
    let size = Size::new(1400.0, 100.0);
    // Inner container, cells row, the cell, its button.
    let messages = click_at(app.list_header(), size, &[0, 0, 0, 0]);
    assert_eq!(messages, ["Sort(Header(Name))"]);
    let messages = click_at(app.list_header(), size, &[0, 0, 2, 0]);
    assert_eq!(messages, ["Sort(Header(Playtime))"]);
    let messages = click_at(app.list_header(), size, &[0, 0, 3, 0]);
    assert_eq!(messages, ["Sort(Header(LastPlayed))"]);
    let messages = click_at(app.list_header(), size, &[0, 0, 4, 0]);
    assert_eq!(messages, ["Sort(Header(Source))"]);
    let messages = click_at(app.list_header(), size, &[0, 0, 5, 0]);
    assert_eq!(messages, ["Sort(Header(Tags))"]);
    assert!(click_at(app.list_header(), size, &[0, 0, 1]).is_empty());
    // Date added stays in the menu.
    let _ = app.update(Message::Sort(SortEvent::Pick(SortKey::Added)));
    assert!(
        Column::ALL
            .iter()
            .all(|column| column.sort() != Some(SortKey::Added))
    );
}

/// A list of four games whose sources and tags differ: `b` is Steam with
/// rpg and co-op, `a` is GOG with puzzle, `d` is Native with rpg, and `c`
/// has no tags and a plain Windows source.
fn labeled(app: &mut App) {
    let mut cards = Vec::new();
    for (name, steam, gog, native, tags) in [
        ("a", false, Some("1"), false, vec!["puzzle"]),
        ("b", true, None, false, vec!["rpg", "co-op"]),
        ("c", false, None, false, vec![]),
        ("d", false, None, true, vec!["rpg"]),
    ] {
        let mut card = card(name, State::Inactive, 60);
        card.source = Source::new(Facts {
            steam,
            gog_id: gog,
            native,
            ..Facts::default()
        });
        card.tags = tags.into_iter().map(str::to_owned).collect();
        cards.push(card);
    }
    let _ = app.update(Message::Loaded(Ok(cards)));
    app.layout = ViewLayout::List;
    app.snap_grid();
}

#[test]
fn the_source_and_tags_headers_sort_flip_and_show_the_indicator() {
    let (_root, mut app) = fixture();
    labeled(&mut app);
    // Source A to Z: GOG, Native, Steam, Windows.
    let _ = app.update(Message::Sort(SortEvent::Header(SortKey::Source)));
    assert_eq!(
        (app.query.sort, app.query.descending),
        (SortKey::Source, false)
    );
    assert_eq!(shown(&app), ["a", "d", "b", "c"]);
    let _ = app.update(Message::Sort(SortEvent::Header(SortKey::Source)));
    assert!(app.query.descending);
    assert_eq!(shown(&app), ["c", "b", "d", "a"]);
    // Tags A to Z: co-op+rpg, puzzle, rpg, then the untagged game last
    // in both directions.
    let _ = app.update(Message::Sort(SortEvent::Header(SortKey::Tags)));
    assert_eq!(
        (app.query.sort, app.query.descending),
        (SortKey::Tags, false)
    );
    assert_eq!(shown(&app), ["b", "a", "d", "c"]);
    let _ = app.update(Message::Sort(SortEvent::Header(SortKey::Tags)));
    assert!(app.query.descending);
    assert_eq!(shown(&app), ["d", "a", "b", "c"]);
    // The header builds with the arrow on the sorted column.
    let _ = app.list_header();
}

#[test]
fn a_resort_moves_the_rows_through_the_motion() {
    let (_root, mut app) = listed(6);
    let before = app.grid_places();
    let _ = app.update(Message::Sort(SortEvent::Header(SortKey::Name)));
    assert!(app.query.descending);
    let after = app.grid_places();
    assert_eq!(after.names.first(), before.names.last());
    assert!(app.grid_motion.active(), "rows spring to their new places");
    let _ = crate::tests::settle_grid(&mut app);
    assert_eq!(app.grid_motion.shown(&after), after.slots);
}

#[test]
fn arrow_keys_walk_the_rows_one_at_a_time() {
    let (_root, mut app) = listed(5);
    key(&mut app, Named::ArrowDown);
    assert_eq!(app.selected.as_deref(), Some("game-000"));
    for expected in ["game-001", "game-002"] {
        key(&mut app, Named::ArrowDown);
        assert_eq!(app.selected.as_deref(), Some(expected));
    }
    key(&mut app, Named::ArrowUp);
    assert_eq!(app.selected.as_deref(), Some("game-001"));
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.selected.as_deref(), Some("game-002"));
    key(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected.as_deref(), Some("game-001"));
}

#[test]
fn a_random_pick_lands_on_a_row_and_scrolls_to_it() {
    let (_root, mut app) = listed(60);
    let last = shown(&app).pop().unwrap();
    for card in &mut app.cards {
        if card.name != last {
            card.error = Some("unreadable".into());
        }
    }
    assert_eq!(app.scroll_offset, 0.0);
    let task = app.update(Message::Random(crate::random_pick::Event::Pick));
    answer(&mut app, task);
    let task = app.update(Message::GridTick(
        app.grid_motion.generation(),
        Instant::now() + Duration::from_secs(1),
    ));
    answer(&mut app, task);
    assert_eq!(app.selected.as_deref(), Some(last.as_str()));
    let places = app.grid_places();
    let slot = places.slots[places.index(&last).unwrap()];
    assert!(app.scroll_offset > 0.0, "the list stayed at the top");
    assert!(slot.y >= app.scroll_offset);
    assert!(slot.y + slot.height <= app.scroll_offset + app.viewport_height + 0.5);
}

#[test]
fn the_list_has_no_collapse() {
    let (_root, mut app) = listed(6);
    app.start_collapse(Instant::now());
    assert!(app.collapse.idle());
}

#[test]
fn overlong_text_ends_in_an_ellipsis_within_its_column() {
    use crate::tile_title::line_width;
    let (_root, mut app) = fixture();
    let mut long = card(
        "a-very-long-environment-name-for-a-game",
        State::SteamRunning,
        0,
    );
    long.display_title = "The Elder Scrolls V: Skyrim Special Anniversary Edition Deluxe".into();
    long.tags = ["rpg", "open-world", "fantasy", "single-player", "modded"]
        .map(String::from)
        .into();
    let short = card("short", State::Inactive, 0);
    let _ = app.update(Message::Loaded(Ok(vec![long, short])));
    for width in [500.0, 700.0, 1000.0] {
        let long = facts(&app, "a-very-long-environment-name-for-a-game");
        let text = row_text(&long, width);
        let room = title_room(&long, width);
        assert!(text.title.ends_with("..."), "{width}: {}", text.title);
        assert!(line_width(&text.title, 14.0, SEMIBOLD) <= room, "{width}");
        let identifier = text.identifier.unwrap();
        assert!(identifier.ends_with("..."), "{width}: {identifier}");
        assert!(line_width(&identifier, 12.0, Font::DEFAULT) <= room);
        assert_eq!(long.state, "running (started from Steam)");
        assert!(text.state.ends_with("..."), "{}", text.state);
        assert!(line_width(&text.state, 13.0, Font::DEFAULT) <= STATE_ROOM);
        for (column, value) in &text.cells {
            assert!(line_width(value, 13.0, Font::DEFAULT) <= column.width());
            if *column == Column::Tags {
                assert!(value.ends_with("..."), "{value}");
            }
        }
        // What fits stays whole.
        let short = row_text(&facts(&app, "short"), width);
        assert_eq!(short.title, "short");
        assert_eq!(short.state, "inactive");
    }
}

#[test]
fn headers_and_the_star_show_hover_and_press() {
    use iced::widget::button::Status;
    let (_root, app) = fixture();
    let palette = app.palette;
    let idle = header_style(palette, Status::Active, false);
    let hovered = header_style(palette, Status::Hovered, false);
    let pressed = header_style(palette, Status::Pressed, false);
    assert_eq!(idle.text_color, palette.muted);
    assert_eq!(
        header_style(palette, Status::Active, true).text_color,
        palette.foreground
    );
    assert_eq!(hovered.text_color, palette.foreground);
    assert_ne!(hovered.background, idle.background);
    assert_ne!(pressed.background, hovered.background);
    assert_eq!(pressed.border.color, palette.primary);
    let idle = star_style(palette, Status::Active);
    let hovered = star_style(palette, Status::Hovered);
    let pressed = star_style(palette, Status::Pressed);
    assert_ne!(hovered.background, idle.background);
    assert_ne!(pressed.background, hovered.background);
    assert_eq!(pressed.border.color, palette.primary);
}

#[test]
fn the_row_hover_and_star_follow_the_posters_fade_and_pop() {
    let (_root, mut app) = listed(3);
    let _ = app.update(Message::Hover(Some("game-001".into())));
    let start = facts(&app, "game-001");
    assert_eq!(start.star, Star::Outline);
    // The first frame starts the fade's clock.
    let now = Instant::now();
    let _ = app.update(Message::HoverTick(app.hover_fades.generation(), now));
    let _ = app.update(Message::HoverTick(
        app.hover_fades.generation(),
        now + Duration::from_millis(30),
    ));
    let rising = facts(&app, "game-001");
    assert!(rising.hover > 0.0 && rising.hover < 1.0, "{}", rising.hover);
    // The star fades in with it, as the poster's does, rather than at once.
    assert_eq!(rising.star, Star::Outline);
    assert_eq!(star_strength(rising.star, rising.hover), rising.hover);
    // The pointer leaves; the hover fades rather than vanishing.
    let _ = app.update(Message::Unhover("game-001".into()));
    let leaving = facts(&app, "game-001");
    assert!(!leaving.hovered);
    assert!(app.hover_fades.fading("game-001"));
    assert!(leaving.hover > 0.0);
    assert_eq!(leaving.star, Star::Outline);
    let _ = app.update(Message::Hover(Some("game-001".into())));
    let _ = app.update(Message::Mark(crate::library_actions::Event::PosterStar(
        "game-001".into(),
    )));
    assert!(app.star_pops.progress("game-001").is_some());
    let now = Instant::now();
    let _ = app.update(Message::PopTick(app.star_pops.generation(), now));
    let _ = app.update(Message::PopTick(
        app.star_pops.generation(),
        now + Duration::from_millis(60),
    ));
    let popping = facts(&app, "game-001");
    assert_eq!(popping.star, Star::Filled);
    assert!(popping.lift > 0.0, "{}", popping.lift);
}
