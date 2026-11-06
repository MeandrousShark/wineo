//! Grouping the library into sections (plan 0062, questions 11 to 13, 20,
//! and 21) and the header slots they lay out.

use std::collections::BTreeSet;

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::{Rectangle, Size};
use jiff::Timestamp;
use wineo_core::library::{GroupBy, SortKey};
use wineo_core::paths::Paths;
use wineo_core::runtime::State;
use wineo_core::source::{Facts, Source};

use crate::grid_motion::{self, GridLayout, HEADER_HEIGHT, PADDING, Places, ViewLayout};
use crate::keys::{Direction, move_index, walk};
use crate::library::Card;
use crate::order::{self, Filters, Of, Order, Query, RECENT};
use crate::slot::{SectionKey, SlotId};
use crate::sort_control::Event;
use crate::tests::{art_file, card, settle_grid, steam_card};
use crate::{App, GAP, Message, artwork};

pub(crate) fn tagged(name: &str, tags: &[&str]) -> Card {
    let mut card = card(name, State::Inactive, 0);
    card.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    card
}

fn at(seconds: i64) -> Option<Timestamp> {
    Some(Timestamp::from_second(1_700_000_000 + seconds).unwrap())
}

/// `card` last played `seconds` after a fixed moment.
pub(crate) fn played(mut card: Card, seconds: i64) -> Card {
    card.last_played_at = at(seconds);
    card
}

fn tag(name: &str) -> SectionKey {
    SectionKey::Tag(name.to_owned())
}

/// The sections `cards` fall into, by key, with their games' names.
fn sections_of(
    cards: &[Card],
    group: GroupBy,
    query: Query,
    filters: &Filters,
) -> Vec<(SectionKey, Vec<String>)> {
    let collapsed = BTreeSet::new();
    let of = Of {
        cards,
        search: "",
        query,
        filters,
        group,
        collapsed: &collapsed,
    };
    Order::default()
        .sections(of)
        .iter()
        .map(|section| {
            (
                section.key.clone(),
                section
                    .cards
                    .iter()
                    .map(|&index| cards[index].name.clone())
                    .collect(),
            )
        })
        .collect()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

/// An app showing `cards`, grouped by `group`, in a window wide enough for
/// a few columns.
pub(crate) fn grouped(cards: Vec<Card>, group: GroupBy) -> App {
    let mut app = crate::tests::app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    app.group = group;
    app.merge(cards);
    app.reorder();
    app.snap_grid();
    app
}

fn slot(section: SectionKey, name: &str) -> SlotId {
    SlotId::new(section, name)
}

#[test]
fn group_by_none_is_one_flat_section_with_hidden_games_in_place_as_today() {
    // Plan 0062, question 21's A: with no grouping there's no Recently
    // played or Hidden, even with Show hidden on.
    let mut hidden = played(tagged("hidden", &["rpg"]), 50);
    hidden.hidden = true;
    let cards = vec![
        played(tagged("hades", &["rpg", "co-op"]), 10),
        hidden,
        tagged("notepad", &[]),
    ];
    let filters = Filters {
        show_hidden: true,
        ..Filters::default()
    };
    let query = Query::default();
    let flat = sections_of(&cards, GroupBy::None, query, &filters);
    assert_eq!(
        flat,
        [(SectionKey::All, names(&["hades", "hidden", "notepad"]))]
    );

    // The app's slots are the flat library's, one per game, with no
    // headers, exactly as `Places::new` measures them.
    let app = grouped(cards, GroupBy::None);
    let places = app.grid_places();
    assert!(places.headers.is_empty());
    let layout = app.grid_layout();
    let flat_places = Places::new(
        app.filtered()
            .iter()
            .map(|card| SlotId::new(SectionKey::All, card.name.as_str()))
            .collect(),
        &app.filtered()
            .iter()
            .map(|card| crate::grid::card_height(card, layout))
            .collect::<Vec<_>>(),
        &vec![layout.card_width(); app.filtered().len()],
        app.columns(),
        app.grid_shift(),
        layout,
        ViewLayout::Grid,
    );
    assert_eq!(*places, flat_places);
    assert!(
        places.ids.iter().all(|slot| slot.widget_id()
            == iced::widget::Id::from(format!("library-card:{}", slot.name())))
    );
}

#[test]
fn a_game_with_several_tags_sits_in_each_with_no_tag_last() {
    // Plan 0062, question 12's A and question 13's A.
    let cards = vec![
        tagged("hades", &["rpg", "co-op"]),
        tagged("celeste", &["platformer"]),
        tagged("notepad", &[]),
        tagged("balatro", &["Cards", "rpg"]),
    ];
    let query = Query {
        sort: SortKey::Name,
        ..Query::default()
    };
    assert_eq!(
        sections_of(&cards, GroupBy::Tag, query, &Filters::default()),
        [
            (tag("Cards"), names(&["balatro"])),
            (tag("co-op"), names(&["hades"])),
            (tag("platformer"), names(&["celeste"])),
            (tag("rpg"), names(&["balatro", "hades"])),
            (SectionKey::NoTag, names(&["notepad"])),
        ]
    );
    // The sort runs inside each section, so Z to A flips rpg's games.
    let descending = Query {
        sort: SortKey::Name,
        descending: true,
        ..Query::default()
    };
    let rpg = sections_of(&cards, GroupBy::Tag, descending, &Filters::default())
        .into_iter()
        .find(|(key, _)| *key == tag("rpg"))
        .unwrap();
    assert_eq!(rpg.1, names(&["hades", "balatro"]));
    // Favorites first applies inside a section too.
    let mut favored = cards.clone();
    favored[0].favorite = true;
    let first = Query {
        sort: SortKey::Name,
        favorites_first: true,
        ..Query::default()
    };
    let rpg = sections_of(&favored, GroupBy::Tag, first, &Filters::default())
        .into_iter()
        .find(|(key, _)| *key == tag("rpg"))
        .unwrap();
    assert_eq!(rpg.1, names(&["hades", "balatro"]));
    // A filter that leaves a section empty drops it.
    let filters = Filters {
        not_tags: BTreeSet::from(["platformer".to_owned()]),
        ..Filters::default()
    };
    let keys: Vec<SectionKey> = sections_of(&cards, GroupBy::Tag, query, &filters)
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert!(!keys.contains(&tag("platformer")), "{keys:?}");
    // With every game tagged, No tag doesn't show.
    let keys: Vec<SectionKey> = sections_of(&cards[..1], GroupBy::Tag, query, &Filters::default())
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys, [tag("co-op"), tag("rpg")]);
}

#[test]
fn sources_run_a_to_z_by_store_else_platform_with_no_source_last() {
    // Plan 0062, question 11: a source is 0054's store, else platform.
    let mut hades = card("hades", State::Inactive, 0);
    hades.source = Source::new(Facts {
        steam: true,
        ..Facts::default()
    });
    let mut papers = card("papers", State::Inactive, 0);
    papers.source = Source::new(Facts {
        gog_id: Some("1"),
        ..Facts::default()
    });
    let mut balatro = card("balatro", State::Inactive, 0);
    balatro.source = Source::new(Facts {
        native: true,
        ..Facts::default()
    });
    // Every readable game has a platform, so only a source with neither
    // falls through to No source.
    let mut unknown = card("unknown", State::Inactive, 0);
    unknown.source = Source {
        store: None,
        platform: None,
        ..unknown.source
    };
    let cards = vec![hades, papers, balatro, unknown];
    let sections = sections_of(
        &cards,
        GroupBy::Source,
        Query::default(),
        &Filters::default(),
    );
    let keys: Vec<&SectionKey> = sections.iter().map(|(key, _)| key).collect();
    let labels: Vec<&str> = cards
        .iter()
        .filter_map(|card| card.source.label())
        .collect();
    assert_eq!(labels.len(), 3, "{labels:?}");
    let mut expected: Vec<SectionKey> = labels
        .iter()
        .map(|label| SectionKey::Source(label))
        .collect();
    expected.sort_by_key(|key| key.title().to_lowercase());
    expected.push(SectionKey::NoSource);
    assert_eq!(keys, expected.iter().collect::<Vec<_>>());
    assert_eq!(sections.last().unwrap().1, names(&["unknown"]));
    // A game has one source, so none shows twice.
    let total: usize = sections.iter().map(|(_, games)| games.len()).sum();
    assert_eq!(total, cards.len());
}

#[test]
fn states_are_running_then_everything_else_then_not_installed() {
    // Plan 0062, question 11.
    let cards = vec![
        steam_card("absent", wineo_core::steam::AppState::NotInstalled),
        card("idle", State::Inactive, 0),
        card("playing", State::Running, 0),
        steam_card("installed", wineo_core::steam::AppState::Installed),
    ];
    assert_eq!(
        sections_of(
            &cards,
            GroupBy::State,
            Query::default(),
            &Filters::default()
        ),
        [
            (SectionKey::Running, names(&["playing"])),
            (SectionKey::OtherState, names(&["idle", "installed"])),
            (SectionKey::NotInstalled, names(&["absent"])),
        ]
    );
}

#[test]
fn only_grouping_by_state_reads_a_cards_state_for_the_order() {
    // A run starting or Steam installing changes no sort input, so other
    // groupings keep the rescan's shortcut (plan 0062, Grouping).
    let idle = card("hades", State::Inactive, 0);
    let mut running = idle.clone();
    running.state = State::Running;
    let absent = steam_card("portal", wineo_core::steam::AppState::NotInstalled);
    let installed = steam_card("portal", wineo_core::steam::AppState::Installed);
    for group in [GroupBy::None, GroupBy::Tag, GroupBy::Source] {
        assert!(order::same_inputs(&idle, &running, group), "{group:?}");
        assert!(order::same_inputs(&absent, &installed, group), "{group:?}");
    }
    assert!(!order::same_inputs(&idle, &running, GroupBy::State));
    assert!(!order::same_inputs(&absent, &installed, GroupBy::State));
    // A change within one State section keeps the shortcut there too.
    let mut stopped = idle.clone();
    stopped.state = State::Stopped;
    assert!(order::same_inputs(&idle, &stopped, GroupBy::State));
}

#[test]
fn a_steam_install_moves_a_game_between_state_sections_on_a_rescan() {
    let absent = steam_card("portal", wineo_core::steam::AppState::NotInstalled);
    let mut app = grouped(
        vec![absent, card("idle", State::Inactive, 0)],
        GroupBy::State,
    );
    let before = app.grid_places();
    assert_eq!(
        before.ids,
        [
            slot(SectionKey::OtherState, "idle"),
            slot(SectionKey::NotInstalled, "portal")
        ]
    );
    let rebuilds = app.order.rebuilds;
    app.merge(vec![
        steam_card("portal", wineo_core::steam::AppState::Installed),
        card("idle", State::Inactive, 0),
    ]);
    assert_eq!(app.order.rebuilds, rebuilds + 1);
    let after = app.grid_places();
    assert_eq!(
        after.ids,
        [
            slot(SectionKey::OtherState, "idle"),
            slot(SectionKey::OtherState, "portal")
        ]
    );
    assert_eq!(
        after
            .headers
            .iter()
            .map(|place| place.header.key.clone())
            .collect::<Vec<_>>(),
        [SectionKey::OtherState]
    );
    // A run starting moves it to Running, with nothing else changed.
    let mut running = steam_card("portal", wineo_core::steam::AppState::Installed);
    running.state = State::SteamRunning;
    app.merge(vec![running, card("idle", State::Inactive, 0)]);
    assert_eq!(
        app.grid_places().ids,
        [
            slot(SectionKey::Running, "portal"),
            slot(SectionKey::OtherState, "idle")
        ]
    );
    // A quiet rescan finds nothing new and keeps the order.
    let rebuilds = app.order.rebuilds;
    let same = app.cards.clone();
    app.merge(same);
    assert_eq!(app.order.rebuilds, rebuilds);
}

#[test]
fn recently_played_holds_the_last_ten_newest_first_whatever_the_sort() {
    // Plan 0062, question 20's A and question 21's A.
    let recent = |count: usize| -> Vec<Card> {
        (0..count)
            .map(|index| played(tagged(&format!("game-{index:02}"), &["rpg"]), index as i64))
            .collect()
    };
    let query = Query {
        sort: SortKey::Name,
        ..Query::default()
    };
    // None played: no section at all.
    let never = vec![tagged("hades", &["rpg"])];
    let keys: Vec<SectionKey> = sections_of(&never, GroupBy::Tag, query, &Filters::default())
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys, [tag("rpg")]);
    // Fewer than ten: all of them, newest first, and each stays in its
    // usual section too.
    let mut three = recent(3);
    three.push(tagged("never", &["rpg"]));
    let sections = sections_of(&three, GroupBy::Tag, query, &Filters::default());
    assert_eq!(
        sections[0],
        (
            SectionKey::Recent,
            names(&["game-02", "game-01", "game-00"])
        )
    );
    assert_eq!(
        sections[1],
        (
            tag("rpg"),
            names(&["game-00", "game-01", "game-02", "never"])
        )
    );
    // Exactly ten.
    let ten = sections_of(&recent(RECENT), GroupBy::Tag, query, &Filters::default());
    assert_eq!(ten[0].1.len(), RECENT);
    // An eleventh pushes the oldest out.
    let eleven = sections_of(
        &recent(RECENT + 1),
        GroupBy::Tag,
        query,
        &Filters::default(),
    );
    assert_eq!(eleven[0].1.len(), RECENT);
    assert_eq!(eleven[0].1.first().unwrap(), "game-10");
    assert!(!eleven[0].1.contains(&"game-00".to_owned()));
    // The sort doesn't reach it: least played first still lists the
    // newest first.
    let least = Query {
        sort: SortKey::LastPlayed,
        descending: false,
        ..Query::default()
    };
    let sections = sections_of(&recent(3), GroupBy::Tag, least, &Filters::default());
    assert_eq!(sections[0].1, names(&["game-02", "game-01", "game-00"]));
    assert_eq!(sections[1].1, names(&["game-00", "game-01", "game-02"]));
    // A filter narrows it like any section: the next newest fills in.
    let mut mixed = recent(RECENT + 1);
    mixed[10].tags = vec!["vr".to_owned()];
    let filters = Filters {
        not_tags: BTreeSet::from(["vr".to_owned()]),
        ..Filters::default()
    };
    let sections = sections_of(&mixed, GroupBy::Tag, query, &filters);
    assert_eq!(sections[0].1.len(), RECENT);
    assert_eq!(sections[0].1.first().unwrap(), "game-09");
    assert!(sections[0].1.contains(&"game-00".to_owned()));
    // It sits first under every grouping.
    for group in [GroupBy::Source, GroupBy::State] {
        let sections = sections_of(&recent(2), group, query, &Filters::default());
        assert_eq!(sections[0].0, SectionKey::Recent, "{group:?}");
    }
}

#[test]
fn a_game_joins_recently_played_when_its_session_ends() {
    let mut app = grouped(
        vec![tagged("hades", &["rpg"]), tagged("celeste", &["rpg"])],
        GroupBy::Tag,
    );
    assert!(
        app.grid_places()
            .ids
            .iter()
            .all(|slot| slot.section() == &tag("rpg"))
    );
    // The rescan after a session reads a new last played time.
    app.merge(vec![
        played(tagged("hades", &["rpg"]), 5),
        tagged("celeste", &["rpg"]),
    ]);
    assert_eq!(
        app.grid_places().ids,
        [
            slot(SectionKey::Recent, "hades"),
            slot(tag("rpg"), "celeste"),
            slot(tag("rpg"), "hades"),
        ]
    );
    // A Steam game whose time becomes unknown leaves, and comes back once
    // it reads again.
    app.merge(vec![tagged("hades", &["rpg"]), tagged("celeste", &["rpg"])]);
    assert!(
        !app.grid_places()
            .ids
            .iter()
            .any(|slot| slot.section() == &SectionKey::Recent)
    );
    app.merge(vec![
        played(tagged("hades", &["rpg"]), 5),
        tagged("celeste", &["rpg"]),
    ]);
    assert_eq!(app.grid_places().ids[0], slot(SectionKey::Recent, "hades"));
}

#[test]
fn hidden_shows_last_and_alone_only_with_show_hidden_on() {
    // Plan 0062, question 21: a hidden game sits only in Hidden, after No
    // tag, and stays out of Recently played.
    let mut hidden = played(tagged("secret", &["rpg"]), 99);
    hidden.hidden = true;
    let cards = vec![
        played(tagged("hades", &["rpg"]), 1),
        hidden,
        tagged("notepad", &[]),
    ];
    let off = sections_of(&cards, GroupBy::Tag, Query::default(), &Filters::default());
    assert_eq!(
        off,
        [
            (SectionKey::Recent, names(&["hades"])),
            (tag("rpg"), names(&["hades"])),
            (SectionKey::NoTag, names(&["notepad"])),
        ]
    );
    let on = Filters {
        show_hidden: true,
        ..Filters::default()
    };
    let sections = sections_of(&cards, GroupBy::Tag, Query::default(), &on);
    assert_eq!(
        sections,
        [
            (SectionKey::Recent, names(&["hades"])),
            (tag("rpg"), names(&["hades"])),
            (SectionKey::NoTag, names(&["notepad"])),
            (SectionKey::Hidden, names(&["secret"])),
        ]
    );
}

#[test]
fn hiding_or_unhiding_a_game_while_grouped_moves_it_to_or_from_hidden() {
    let mut app = grouped(
        vec![tagged("hades", &["rpg"]), tagged("celeste", &["rpg"])],
        GroupBy::Tag,
    );
    app.filters.show_hidden = true;
    app.reorder();
    if let Some(card) = app.cards.iter_mut().find(|card| card.name == "hades") {
        card.hidden = true;
    }
    app.order.cards_edited();
    app.reorder();
    assert_eq!(
        app.grid_places().ids,
        [
            slot(tag("rpg"), "celeste"),
            slot(SectionKey::Hidden, "hades")
        ]
    );
    if let Some(card) = app.cards.iter_mut().find(|card| card.name == "hades") {
        card.hidden = false;
    }
    app.order.cards_edited();
    app.reorder();
    assert_eq!(
        app.grid_places().ids,
        [slot(tag("rpg"), "celeste"), slot(tag("rpg"), "hades")]
    );
}

#[test]
fn a_header_takes_a_row_of_its_own_and_its_cards_start_a_new_one() {
    let layout = GridLayout::default();
    let width = layout.card_width();
    let ids: Vec<SlotId> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| SlotId::new(SectionKey::All, *name))
        .collect();
    let heights = vec![layout.poster_height(); 4];
    let headers = vec![
        grid_motion::Header {
            key: tag("one"),
            count: 1,
            collapsed: false,
            at: 0,
        },
        grid_motion::Header {
            key: tag("two"),
            count: 3,
            collapsed: false,
            at: 1,
        },
    ];
    let places = Places::sectioned(
        ids,
        &heights,
        &[width; 4],
        headers,
        width,
        3,
        10.0,
        layout,
        ViewLayout::Grid,
    );
    let row = layout.poster_height();
    let first = &places.headers[0].slot;
    assert_eq!(first.position(), iced::Point::new(PADDING + 10.0, PADDING));
    // Every column wide, gaps included.
    assert_eq!(
        first.size(),
        Size::new(3.0 * width + 2.0 * GAP, HEADER_HEIGHT)
    );
    let cards_top = PADDING + HEADER_HEIGHT + GAP;
    assert_eq!(places.slots[0].y, cards_top);
    // One card leaves the rest of its row empty; the next header starts
    // below it.
    let second = places.headers[1].slot.y;
    assert_eq!(second, cards_top + row + GAP);
    let next = second + HEADER_HEIGHT + GAP;
    assert_eq!(
        places.slots[1].position(),
        iced::Point::new(PADDING + 10.0, next)
    );
    assert_eq!(places.slots[3].y, next);
    assert_eq!(places.height, next + row + PADDING);
    // The card below one is in its own section only.
    assert_eq!(places.below, [None, None, None, None]);

    // A list's header is a row across the list.
    let rows = Places::sectioned(
        vec![SlotId::new(SectionKey::All, "a")],
        &[crate::list_view::ROW_HEIGHT],
        &[700.0],
        vec![grid_motion::Header {
            key: SectionKey::Recent,
            count: 1,
            collapsed: false,
            at: 0,
        }],
        700.0,
        1,
        0.0,
        layout,
        ViewLayout::List,
    );
    assert_eq!(
        rows.headers[0].slot,
        Rectangle::new(
            iced::Point::new(PADDING, PADDING),
            Size::new(700.0, HEADER_HEIGHT)
        )
    );
    assert_eq!(
        rows.slots[0].y,
        PADDING + HEADER_HEIGHT + grid_motion::ROW_GAP
    );
}

#[test]
fn the_rows_in_view_are_found_the_same_way_with_or_without_sections() {
    // The library collapse path reads them from the slots now.
    let layout = GridLayout::default();
    let heights: Vec<f32> = (0..23)
        .map(|index| layout.poster_height() + if index % 4 == 1 { 40.0 } else { 0.0 })
        .collect();
    let ids: Vec<SlotId> = (0..23)
        .map(|index| SlotId::new(SectionKey::All, format!("game-{index}")))
        .collect();
    let places = Places::new(
        ids,
        &heights,
        &[layout.card_width(); 23],
        4,
        0.0,
        layout,
        ViewLayout::Grid,
    );
    for (top, bottom) in [
        (0.0, 300.0),
        (250.0, 900.0),
        (1000.0, 1001.0),
        (5000.0, 6000.0),
    ] {
        assert_eq!(
            places.between(top, bottom),
            grid_motion::cards_between(&heights, 4, top, bottom),
            "{top}..{bottom}"
        );
    }
}

#[test]
fn art_loads_through_each_slots_card_and_skips_headers() {
    // Two long sections, so the second's cards sit far below a header the
    // first's cards don't have.
    let cards: Vec<Card> = (0..80)
        .map(|index| {
            let section = if index < 40 { "a" } else { "b" };
            let mut card = tagged(&format!("game-{index:02}"), &[section]);
            card.art = Some(art_file(&format!("/tmp/art-{index}.png"), index as u64));
            card
        })
        .collect();
    let mut app = grouped(cards, GroupBy::Tag);
    let places = app.grid_places();
    assert_eq!(places.headers.len(), 2);
    // Scrolled to the second section's middle.
    let target = places
        .ids
        .iter()
        .position(|slot| slot.name() == "game-60")
        .unwrap();
    app.scroll_offset = places.slots[target].y;
    let _ = app.load_visible_art();
    let near: Vec<&str> = app
        .cards
        .iter()
        .filter(|card| {
            let key = (card.art.clone().unwrap(), artwork::Kind::Grid);
            app.visible_art.contains(&key)
        })
        .map(|card| card.name.as_str())
        .collect();
    assert!(near.contains(&"game-60"), "{near:?}");
    // Each key is the card drawn in that slot: only the second section's
    // cards are near, none of the first's.
    assert!(
        near.iter().all(|name| name.as_bytes()[5] >= b'4'),
        "{near:?}"
    );
    // Every visible slot's own art is asked for, header or not above it.
    let shown = app.grid_motion.shown(&places);
    let slots = app.slot_cards(&places);
    for (index, bounds) in shown.iter().enumerate() {
        if bounds.y <= app.scroll_offset + app.viewport_height
            && bounds.y + bounds.height >= app.scroll_offset
        {
            let key = (slots[index].art.clone().unwrap(), artwork::Kind::Grid);
            assert!(app.visible_art.contains(&key), "{}", slots[index].name);
        }
    }
}

#[test]
fn slot_cards_follow_the_slots_when_sections_reorder_every_game_once() {
    // Grouped by Source, each game has one slot, as many as the flat
    // library, but in another order.
    let mut steam = card("a-steam", State::Inactive, 0);
    steam.source = Source::new(Facts {
        steam: true,
        ..Facts::default()
    });
    let mut native = card("b-native", State::Inactive, 0);
    native.source = Source::new(Facts {
        native: true,
        ..Facts::default()
    });
    let app = grouped(vec![steam, native], GroupBy::Source);
    let places = app.grid_places();
    let names: Vec<&str> = app
        .slot_cards(&places)
        .iter()
        .map(|card| card.name.as_str())
        .collect();
    let slots: Vec<&str> = places.ids.iter().map(SlotId::name).collect();
    assert_eq!(names, slots);
}

/// What `Placed` finds: the viewport and each container's bounds by id.
type Found = (Option<Rectangle>, Vec<(iced::widget::Id, Rectangle)>);

/// The library scrollable's viewport and the bounds of every container
/// with one of `ids`, by id.
struct Placed {
    ids: Vec<iced::widget::Id>,
    found: std::sync::Arc<std::sync::Mutex<Found>>,
}

impl iced::advanced::widget::Operation for Placed {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: Rectangle) {
        if let Some(id) = id.filter(|id| self.ids.contains(id)) {
            self.found.lock().unwrap().1.push((id.clone(), bounds));
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        _translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&iced::widget::Id::new(crate::SCROLL_ID)) {
            self.found.lock().unwrap().0 = Some(bounds);
        }
    }
}

#[test]
fn the_window_draws_headers_and_copies_where_the_app_places_them() {
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        let mut app = grouped(
            vec![
                played(tagged("hades", &["rpg", "co-op"]), 3),
                tagged("celeste", &["platformer"]),
                tagged("notepad", &[]),
                tagged("balatro", &["rpg"]),
            ],
            GroupBy::Tag,
        );
        app.layout = layout;
        let _ = app.window_resized(Size::new(1200.0, 800.0));
        app.snap_grid();
        let places = app.grid_places();
        // Three copies of hades: Recently played, co-op, and rpg.
        assert_eq!(
            places
                .ids
                .iter()
                .filter(|slot| slot.name() == "hades")
                .count(),
            3
        );
        let mut ids: Vec<iced::widget::Id> = places.ids.iter().map(SlotId::widget_id).collect();
        ids.extend(
            places
                .headers
                .iter()
                .map(|place| place.header.key.header_id()),
        );
        let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
        let found = std::sync::Arc::default();
        laid.run(Box::new(Placed {
            ids: ids.clone(),
            found: std::sync::Arc::clone(&found),
        }));
        let (viewport, drawn) = std::mem::take(&mut *found.lock().unwrap());
        let viewport = viewport.expect("the library scrollable");
        let at = |id: &iced::widget::Id| {
            let matches: Vec<Rectangle> = drawn
                .iter()
                .filter(|(found, _)| found == id)
                .map(|(_, bounds)| *bounds)
                .collect();
            assert_eq!(matches.len(), 1, "{id:?} in {layout:?}");
            matches[0]
        };
        for (slot, place) in places.ids.iter().zip(&places.slots) {
            let bounds = at(&slot.widget_id());
            assert_eq!(bounds.x - viewport.x, place.x, "{slot:?} {layout:?}");
            assert_eq!(bounds.y - viewport.y, place.y, "{slot:?} {layout:?}");
        }
        for place in &places.headers {
            let bounds = at(&place.header.key.header_id());
            assert_eq!(bounds.x - viewport.x, place.slot.x, "{layout:?}");
            assert_eq!(bounds.y - viewport.y, place.slot.y, "{layout:?}");
            assert_eq!(bounds.width, place.slot.width, "{layout:?}");
            assert_eq!(bounds.height, HEADER_HEIGHT, "{layout:?}");
        }
    }
}

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::Loaded(Ok(vec![
        tagged("hades", &["rpg", "co-op"]),
        tagged("celeste", &["platformer"]),
    ])));
    (root, app)
}

fn finish_group_write(app: &mut App) {
    let group = app.view_writes.group.expect("a group write in flight");
    let field = crate::sort_control::Field::Group(group);
    let result = crate::sort_control::save(&app.paths, &app.shown, field);
    let _ = app.update(Message::Sort(Event::Saved(field, result)));
}

#[test]
fn group_by_is_a_sort_submenu_and_saved_as_group() {
    let (_root, mut app) = fixture();
    crate::sort_control::tests::open_sort(&mut app);
    assert!(app.sort_menu_shown() && !app.group_menu);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    assert!(app.group_menu);
    // Escape closes only the submenu, back to its row.
    let _ = app.update(Message::Key(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(app.sort_menu_shown() && !app.group_menu);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    let _ = app.update(Message::Key(
        Key::Named(Named::ArrowLeft),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(app.sort_menu_shown() && !app.group_menu);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    let _ = app.update(Message::Sort(Event::Group(GroupBy::Tag)));
    assert_eq!(app.group, GroupBy::Tag);
    assert!(!app.sort_menu && !app.group_menu);
    assert_eq!(app.grid_places().headers.len(), 3);
    finish_group_write(&mut app);
    assert!(app.feature_notice.is_none());
    let saved = std::fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(saved.contains("group = \"tag\""), "{saved}");
    // The next launch opens grouped.
    let reopened = App::boot(app.paths.clone()).0;
    assert_eq!(reopened.group, GroupBy::Tag);
    // Picking the same grouping writes nothing.
    crate::sort_control::tests::open_sort(&mut app);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    let _ = app.update(Message::Sort(Event::Group(GroupBy::Tag)));
    assert!(app.view_writes.group.is_none());
    // Back to None is the flat library, saved too.
    crate::sort_control::tests::open_sort(&mut app);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    let _ = app.update(Message::Sort(Event::Group(GroupBy::None)));
    assert!(app.grid_places().headers.is_empty());
    finish_group_write(&mut app);
    let saved = std::fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(saved.contains("group = \"none\""), "{saved}");
}

#[test]
fn the_submenu_opens_beside_its_row_and_flips_left_near_the_edge() {
    let (_root, mut app) = fixture();
    app.window_size = Size::new(1200.0, 800.0);
    crate::sort_control::tests::open_sort(&mut app);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    assert!(app.group_menu_view().is_some());
    // Leaving the menu closes the submenu with it.
    let _ = app.update(Message::Sort(Event::CloseMenu));
    assert!(!app.group_menu);
    assert!(app.group_menu_view().is_none());
}

#[test]
fn two_copies_of_a_multi_tag_game_hover_select_mark_and_hide_as_one_game() {
    let mut app = grouped(
        vec![
            tagged("hades", &["rpg", "co-op"]),
            tagged("celeste", &["rpg"]),
        ],
        GroupBy::Tag,
    );
    let coop = slot(tag("co-op"), "hades");
    let rpg = slot(tag("rpg"), "hades");
    let places = app.grid_places();
    assert_eq!(
        places.ids,
        [coop.clone(), slot(tag("rpg"), "celeste"), rpg.clone()]
    );
    assert_ne!(coop.widget_id(), rpg.widget_id());
    // Hover belongs to one copy.
    let _ = app.update(Message::Hover(Some(rpg.clone())));
    assert_eq!(app.hovered.as_ref(), Some(&rpg));
    assert!(app.hover_fades.fading(&rpg) && !app.hover_fades.fading(&coop));
    let _ = app.update(Message::Unhover(rpg.clone()));
    // A press on the rpg copy selects the game with the cursor there.
    let _ = app.update(Message::Press(rpg.clone()));
    assert_eq!(app.selected.as_deref(), Some("hades"));
    assert_eq!(app.selected_slot(), Some(rpg.clone()));
    assert!(app.panel_open);
    // The other copy moves the cursor and leaves the panel open.
    let _ = app.update(Message::Press(coop.clone()));
    assert!(app.panel_open);
    assert_eq!(app.selected_slot(), Some(coop.clone()));
    // Its own motion: each copy keeps its own body.
    assert_eq!(app.grid_motion.opacity(&coop), 1.0);
    assert_eq!(app.grid_motion.opacity(&rpg), 1.0);
    // A mark is the game's, on both copies.
    app.modifiers = Modifiers::CTRL;
    let _ = app.update(Message::Press(rpg.clone()));
    app.modifiers = Modifiers::empty();
    assert!(app.marked.contains("hades"));
    // Hiding the game takes both copies, and the cursor moves on.
    if let Some(card) = app.cards.iter_mut().find(|card| card.name == "hades") {
        card.hidden = true;
    }
    app.order.cards_edited();
    app.reorder();
    app.reflow_grid();
    assert_eq!(app.grid_places().ids, [slot(tag("rpg"), "celeste")]);
    assert!(app.marked.is_empty());
    let _ = settle_grid(&mut app);
    assert!(!app.grid_motion.active());
}

fn header_places(sections: &[(&str, &[&str])]) -> Places {
    let layout = GridLayout::default();
    let mut ids = Vec::new();
    let mut headers = Vec::new();
    for (section, games) in sections {
        headers.push(grid_motion::Header {
            key: tag(section),
            count: games.len(),
            collapsed: false,
            at: ids.len(),
        });
        ids.extend(games.iter().map(|game| slot(tag(section), game)));
    }
    let count = ids.len();
    Places::sectioned(
        ids,
        &vec![layout.poster_height(); count],
        &vec![layout.card_width(); count],
        headers,
        layout.card_width(),
        3,
        0.0,
        layout,
        ViewLayout::Grid,
    )
}

#[test]
fn headers_spring_with_the_cards_and_fade_in_and_out() {
    let start = std::time::Instant::now();
    let mut motion = grid_motion::GridMotion::default();
    let one = header_places(&[("rpg", &["hades"])]);
    motion.snap_to(&one);
    assert_eq!(motion.header_opacity(&tag("rpg")), 1.0);
    // A section above pushes rpg's header down: it springs from where it
    // was, and the new header fades in where it lands.
    let two = header_places(&[("co-op", &["celeste"]), ("rpg", &["hades"])]);
    motion.spring_to(&two, start);
    assert!(motion.active());
    let rpg = two.headers[1].slot;
    assert_eq!(
        rpg.position() + motion.header_offset(&tag("rpg")),
        one.headers[0].slot.position()
    );
    assert_eq!(motion.header_opacity(&tag("co-op")), 0.0);
    assert_eq!(motion.header_offset(&tag("co-op")), iced::Vector::ZERO);
    // Gone again: it fades out where it was, keeping what it showed, and
    // holds the height meanwhile.
    motion.spring_to(&one, start + std::time::Duration::from_millis(16));
    let leaving: Vec<&SectionKey> = motion
        .leaving_headers()
        .iter()
        .map(|leaving| &leaving.header.key)
        .collect();
    assert_eq!(leaving, [&tag("co-op")]);
    assert_eq!(motion.leaving_headers()[0].header.count, 1);
    assert!(motion.hold().is_some());
    motion.tick(
        motion.generation(),
        start + std::time::Duration::from_secs(2),
    );
    motion.tick(
        motion.generation(),
        start + std::time::Duration::from_secs(3),
    );
    assert!(motion.leaving_headers().is_empty());
    assert!(!motion.active());
    assert_eq!(motion.header_offset(&tag("rpg")), iced::Vector::ZERO);
    // A snap puts every header in place at once.
    motion.spring_to(&two, start + std::time::Duration::from_secs(4));
    motion.snap_to(&one);
    assert!(!motion.active() && motion.leaving_headers().is_empty());
}

#[test]
fn a_tag_edit_that_only_moves_a_game_between_sections_still_moves_the_cards() {
    // The flat order stays the same, but the slots don't.
    let mut app = grouped(
        vec![
            tagged("celeste", &["platformer"]),
            tagged("hades", &["rpg"]),
        ],
        GroupBy::Tag,
    );
    let flat: Vec<String> = app
        .filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect();
    app.marked.insert("celeste".to_owned());
    let _ = app.tag_marked("rpg", true);
    let after: Vec<String> = app
        .filtered()
        .iter()
        .map(|card| card.name.clone())
        .collect();
    assert_eq!(flat, after);
    let new = slot(tag("rpg"), "celeste");
    assert!(app.grid_places().index(&new).is_some());
    // The motion owner heard of the new slot, so it fades in rather than
    // popping up with no body.
    assert!(app.grid_motion.active());
    assert_eq!(app.grid_motion.opacity(&new), 0.0);
}

fn walk_places(rows: &[(&str, &[&str])], columns: usize, mode: ViewLayout) -> Places {
    let layout = GridLayout::default();
    let mut ids = Vec::new();
    let mut headers = Vec::new();
    for (section, games) in rows {
        headers.push(grid_motion::Header {
            key: tag(section),
            count: games.len(),
            collapsed: false,
            at: ids.len(),
        });
        ids.extend(games.iter().map(|game| slot(tag(section), game)));
    }
    let count = ids.len();
    let (height, width) = match mode {
        ViewLayout::Grid => (layout.poster_height(), layout.card_width()),
        ViewLayout::List => (crate::list_view::ROW_HEIGHT, 700.0),
    };
    Places::sectioned(
        ids,
        &vec![height; count],
        &vec![width; count],
        headers,
        width,
        columns,
        0.0,
        layout,
        mode,
    )
}

/// A slot as `section:game`.
fn named(slot: &SlotId) -> String {
    format!("{}:{}", slot.section().title(), slot.name())
}

fn walked(places: &Places, from: &str, direction: Direction) -> String {
    let at = places.ids.iter().position(|slot| named(slot) == from);
    assert!(at.is_some(), "{from}");
    let next = walk(places, at, direction).unwrap();
    named(&places.ids[next])
}

#[test]
fn up_and_down_cross_sections_by_nearest_column_and_left_and_right_follow_shown_order() {
    // Plan 0062, question 13: rows stop lining up across sections.
    let places = walk_places(
        &[
            ("a", &["a1", "a2", "a3", "a4", "a5"]),
            ("b", &["b1", "b2"]),
            ("c", &["c1", "c2", "c3", "c4"]),
        ],
        4,
        ViewLayout::Grid,
    );
    // From a3 and a4, a's last row has only a5, the nearest.
    assert_eq!(walked(&places, "a:a3", Direction::Down), "a:a5");
    assert_eq!(walked(&places, "a:a4", Direction::Down), "a:a5");
    // a5 down into b, past its header, same column.
    assert_eq!(walked(&places, "a:a5", Direction::Down), "b:b1");
    // b2 down to c's first row, same column.
    assert_eq!(walked(&places, "b:b2", Direction::Down), "c:c2");
    // Up from b2 lands on a5, the only card in a's last row.
    assert_eq!(walked(&places, "b:b2", Direction::Up), "a:a5");
    assert_eq!(walked(&places, "c:c4", Direction::Up), "b:b2");
    assert_eq!(walked(&places, "c:c1", Direction::Up), "b:b1");
    assert_eq!(walked(&places, "b:b1", Direction::Up), "a:a5");
    // Nowhere to go stays put.
    assert_eq!(walked(&places, "a:a2", Direction::Up), "a:a2");
    assert_eq!(walked(&places, "c:c3", Direction::Down), "c:c3");
    // Left and Right run in shown order across headers and wrap.
    assert_eq!(walked(&places, "a:a5", Direction::Right), "b:b1");
    assert_eq!(walked(&places, "b:b1", Direction::Left), "a:a5");
    assert_eq!(walked(&places, "c:c4", Direction::Right), "a:a1");
    assert_eq!(walked(&places, "a:a1", Direction::Left), "c:c4");
    // No cursor starts on the first slot; nothing shown goes nowhere.
    assert_eq!(walk(&places, None, Direction::Down), Some(0));
    let empty = walk_places(&[], 4, ViewLayout::Grid);
    assert_eq!(walk(&empty, None, Direction::Down), None);

    // The list is one column, so Up and Down step through the shown order
    // across headers.
    let rows = walk_places(&[("a", &["a1", "a2"]), ("b", &["b1"])], 1, ViewLayout::List);
    assert_eq!(walked(&rows, "a:a2", Direction::Down), "b:b1");
    assert_eq!(walked(&rows, "b:b1", Direction::Up), "a:a2");
    assert_eq!(walked(&rows, "b:b1", Direction::Down), "b:b1");
}

#[test]
fn the_walk_matches_the_old_arithmetic_on_a_flat_grid() {
    // Group by off moves exactly as before.
    let layout = GridLayout::default();
    for columns in 1..=6 {
        for count in 0..=14 {
            let ids: Vec<SlotId> = (0..count)
                .map(|index| SlotId::new(SectionKey::All, format!("game-{index}")))
                .collect();
            let places = Places::new(
                ids,
                &vec![layout.poster_height(); count],
                &vec![layout.card_width(); count],
                columns,
                0.0,
                layout,
                ViewLayout::Grid,
            );
            for direction in [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ] {
                for current in std::iter::once(None).chain((0..count).map(Some)) {
                    assert_eq!(
                        walk(&places, current, direction),
                        move_index(current, count, columns, direction),
                        "{columns} columns, {count} cards, {current:?} {direction:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_arrow_keys_walk_copies_across_sections_in_the_app() {
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        let mut app = grouped(
            vec![
                played(tagged("hades", &["rpg"]), 5),
                tagged("celeste", &["rpg"]),
                tagged("balatro", &["rpg"]),
            ],
            GroupBy::Tag,
        );
        app.layout = layout;
        app.snap_grid();
        let _ = app.update(Message::Press(slot(SectionKey::Recent, "hades")));
        let press = |app: &mut App, named: Named| {
            let _ = app.update(Message::KeyWithFocus(
                Key::Named(named),
                Modifiers::empty(),
                iced::event::Status::Ignored,
                false,
            ));
        };
        // Down from Recently played lands in rpg's first row.
        press(&mut app, Named::ArrowDown);
        let first = app.selected_slot().unwrap();
        assert_eq!(first.section(), &tag("rpg"), "{layout:?}");
        // Right in shown order to rpg's copy of hades: the cursor moves,
        // and the panel stays on the game.
        let rpg_hades = slot(tag("rpg"), "hades");
        while app.selected_slot().as_ref() != Some(&rpg_hades) {
            press(&mut app, Named::ArrowRight);
        }
        assert!(app.panel_open, "{layout:?}");
        assert_eq!(app.selected.as_deref(), Some("hades"));
        // Left from rpg's first slot goes back across the header.
        let places = app.grid_places();
        app.set_cursor(places.ids[1].clone());
        press(&mut app, Named::ArrowLeft);
        assert_eq!(
            app.selected_slot(),
            Some(slot(SectionKey::Recent, "hades")),
            "{layout:?}"
        );
    }
}

#[test]
fn a_game_moving_between_sections_springs_and_its_extra_copies_fade() {
    // Plan 0062, question 17's A, with hades tagged rpg and co-op.
    let start = std::time::Instant::now();
    let flat = |names: &[&str]| -> Places {
        let layout = GridLayout::default();
        Places::new(
            names
                .iter()
                .map(|name| SlotId::new(SectionKey::All, *name))
                .collect(),
            &vec![layout.poster_height(); names.len()],
            &vec![layout.card_width(); names.len()],
            3,
            0.0,
            layout,
            ViewLayout::Grid,
        )
    };
    let none = flat(&["celeste", "hades", "notepad"]);
    let by_tag = header_places(&[
        ("co-op", &["hades"]),
        ("platformer", &["celeste"]),
        ("rpg", &["hades"]),
    ]);
    let mut motion = grid_motion::GridMotion::default();
    motion.snap_to(&none);
    let drawn = |motion: &grid_motion::GridMotion, places: &Places, id: &SlotId| {
        let index = places.index(id).unwrap();
        motion.drawn(places, index).position()
    };
    let old_hades = drawn(&motion, &none, &SlotId::new(SectionKey::All, "hades"));
    let old_celeste = drawn(&motion, &none, &SlotId::new(SectionKey::All, "celeste"));
    motion.spring_to(&by_tag, start);
    // The first copy in shown order springs from the old spot.
    assert_eq!(
        drawn(&motion, &by_tag, &slot(tag("co-op"), "hades")),
        old_hades
    );
    assert_eq!(motion.opacity(&slot(tag("co-op"), "hades")), 1.0);
    // The extra copy is new, so it fades in where it lands.
    assert_eq!(motion.opacity(&slot(tag("rpg"), "hades")), 0.0);
    assert_eq!(
        motion.offset(&slot(tag("rpg"), "hades")),
        iced::Vector::ZERO
    );
    // A game in one section springs to its spot.
    assert_eq!(
        drawn(&motion, &by_tag, &slot(tag("platformer"), "celeste")),
        old_celeste
    );
    // Notepad has no tag here, so it leaves.
    let leaving: Vec<&str> = motion.leaving().iter().map(|card| card.id.name()).collect();
    assert_eq!(leaving, ["notepad"]);

    // Back to None: the first copy springs, the extra fades out.
    motion.snap_to(&by_tag);
    let coop = drawn(&motion, &by_tag, &slot(tag("co-op"), "hades"));
    motion.spring_to(&none, start);
    assert_eq!(
        drawn(&motion, &none, &SlotId::new(SectionKey::All, "hades")),
        coop
    );
    let leaving: Vec<&SlotId> = motion.leaving().iter().map(|card| &card.id).collect();
    assert_eq!(leaving, [&slot(tag("rpg"), "hades")]);
}

#[test]
fn a_copy_that_stays_keeps_its_own_body_when_another_copy_goes() {
    // Hades leaves co-op, above rpg: rpg's copy keeps its place rather
    // than springing from co-op's.
    let start = std::time::Instant::now();
    let both = header_places(&[("co-op", &["hades"]), ("rpg", &["celeste", "hades"])]);
    let rpg_only = header_places(&[("rpg", &["celeste", "hades"])]);
    let mut motion = grid_motion::GridMotion::default();
    motion.snap_to(&both);
    let before = motion
        .drawn(&both, both.index(&slot(tag("rpg"), "hades")).unwrap())
        .position();
    motion.spring_to(&rpg_only, start);
    let index = rpg_only.index(&slot(tag("rpg"), "hades")).unwrap();
    assert_eq!(motion.drawn(&rpg_only, index).position(), before);
    let leaving: Vec<&SlotId> = motion.leaving().iter().map(|card| &card.id).collect();
    assert_eq!(leaving, [&slot(tag("co-op"), "hades")]);
}

#[test]
fn switching_group_by_in_the_app_springs_the_paired_copies() {
    let mut app = grouped(
        vec![
            tagged("hades", &["rpg", "co-op"]),
            tagged("celeste", &["platformer"]),
        ],
        GroupBy::None,
    );
    let _ = settle_grid(&mut app);
    app.group = GroupBy::Tag;
    let _ = app.resorted();
    assert!(app.grid_motion.active());
    assert_eq!(app.grid_motion.opacity(&slot(tag("co-op"), "hades")), 1.0);
    assert_eq!(app.grid_motion.opacity(&slot(tag("rpg"), "hades")), 0.0);
    assert!(app.grid_motion.leaving().is_empty());
    let _ = settle_grid(&mut app);
    app.group = GroupBy::None;
    let _ = app.resorted();
    let leaving: Vec<&SlotId> = app
        .grid_motion
        .leaving()
        .iter()
        .map(|card| &card.id)
        .collect();
    assert_eq!(leaving, [&slot(tag("rpg"), "hades")]);
}
