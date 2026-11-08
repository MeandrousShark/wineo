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
use crate::keys::{Direction, Spot, move_index, walk};
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

/// The pointer moved onto `slot`'s card as drawn, through the grid widget,
/// and what that published handed to the app.
fn point_at(app: &mut App, slot: &SlotId) {
    let places = app.grid_places();
    let at = app
        .grid_motion
        .drawn(&places, places.index(slot).unwrap())
        .center();
    let messages = {
        let mut laid =
            crate::layout_probe::lay_out(app.grid(), Size::new(app.viewport_width, 4000.0));
        laid.event_at(
            &iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: at }),
            iced::mouse::Cursor::Available(at),
        )
    };
    for message in messages {
        let _ = app.update(message);
    }
}

/// A press and release on `slot`'s card as drawn, with `modifiers` held.
fn click_slot(app: &mut App, slot: &SlotId, modifiers: Modifiers) {
    let places = app.grid_places();
    let at = app
        .grid_motion
        .drawn(&places, places.index(slot).unwrap())
        .center();
    let messages = click(app, at);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Press(pressed) if pressed == slot)),
        "{slot:?}: {messages:?}"
    );
    app.modifiers = modifiers;
    for message in messages {
        let _ = app.update(message);
    }
    app.modifiers = Modifiers::empty();
}

#[test]
fn two_copies_of_a_multi_tag_game_hover_select_mark_and_hide_as_one_game() {
    let mut app = grouped(
        vec![
            tagged("hades", &["rpg", "co-op"]),
            tagged("celeste", &["rpg"]),
            tagged("zelda", &["rpg"]),
        ],
        GroupBy::Tag,
    );
    let coop = slot(tag("co-op"), "hades");
    let rpg = slot(tag("rpg"), "hades");
    let places = app.grid_places();
    assert_eq!(
        places.ids,
        [
            coop.clone(),
            slot(tag("rpg"), "celeste"),
            rpg.clone(),
            slot(tag("rpg"), "zelda")
        ]
    );
    assert_ne!(coop.widget_id(), rpg.widget_id());
    // The pointer on the drawn rpg copy hovers that copy alone.
    point_at(&mut app, &rpg);
    assert_eq!(app.hovered.as_ref(), Some(&rpg));
    assert!(app.hover_fades.fading(&rpg) && !app.hover_fades.fading(&coop));
    // A click on it selects the game with the cursor there.
    click_slot(&mut app, &rpg, Modifiers::empty());
    assert_eq!(app.selected.as_deref(), Some("hades"));
    assert_eq!(app.selected_slot(), Some(rpg.clone()));
    assert!(app.panel_open);
    // The other copy moves the cursor and leaves the panel open.
    click_slot(&mut app, &coop, Modifiers::empty());
    assert!(app.panel_open);
    assert_eq!(app.selected_slot(), Some(coop.clone()));
    assert_eq!(app.grid_motion.opacity(&coop), 1.0);
    assert_eq!(app.grid_motion.opacity(&rpg), 1.0);
    // A Ctrl+click marks the game, on both copies.
    click_slot(&mut app, &rpg, Modifiers::CTRL);
    assert!(app.marked.contains("hades"));
    // Back on the rpg copy, the real Hide takes both copies, and the
    // cursor's successor is the slot after it, counting the co-op copy
    // before it that leaves too.
    click_slot(&mut app, &rpg, Modifiers::empty());
    let _ = app.update(Message::Mark(crate::library_actions::Event::Toggle(
        "hades".to_owned(),
        crate::library_actions::Flag::Hidden,
    )));
    assert!(
        app.cards
            .iter()
            .any(|card| card.name == "hades" && card.hidden)
    );
    assert_eq!(
        app.grid_places().ids,
        [slot(tag("rpg"), "celeste"), slot(tag("rpg"), "zelda")]
    );
    assert!(app.marked.is_empty());
    assert_eq!(app.selected_slot(), Some(slot(tag("rpg"), "zelda")));
    let _ = settle_grid(&mut app);
    assert!(!app.grid_motion.active());
}

#[test]
fn the_action_bar_acts_once_on_a_marked_game_shown_twice() {
    let mut app = grouped(
        vec![
            tagged("hades", &["rpg", "co-op"]),
            tagged("celeste", &["rpg"]),
            tagged("zelda", &["rpg"]),
        ],
        GroupBy::Tag,
    );
    click_slot(&mut app, &slot(tag("co-op"), "hades"), Modifiers::CTRL);
    click_slot(&mut app, &slot(tag("rpg"), "celeste"), Modifiers::CTRL);
    // Three marked slots, two games: the bar counts games.
    let places = app.grid_places();
    let marked_slots = places
        .ids
        .iter()
        .filter(|slot| app.marked.contains(slot.name()))
        .count();
    assert_eq!(marked_slots, 3);
    assert!(app.action_bar_shown());
    assert_eq!(
        crate::action_bar::count_label(app.marked.len()),
        "2 selected"
    );
    // Favorite once per game.
    let _ = app.update(Message::Tags(crate::action_bar::Event::ToggleFlag(
        crate::library_actions::Flag::Favorite,
    )));
    let favorites: BTreeSet<&str> = app
        .cards
        .iter()
        .filter(|card| card.favorite)
        .map(|card| card.name.as_str())
        .collect();
    assert_eq!(favorites, BTreeSet::from(["celeste", "hades"]));
    assert_eq!(app.marked.len(), 2);
    // A tag from the bar's list puts hades in another section, keeping
    // its other copies.
    let _ = app.update(Message::Tags(crate::action_bar::Event::ToggleMenu));
    let request = app.tag_opening.pending.expect("a request for the button");
    let _ = app.update(Message::Tags(crate::action_bar::Event::OpenAbove(
        request,
        Some(Rectangle::new(
            iced::Point::new(400.0, 700.0),
            Size::new(80.0, 32.0),
        )),
    )));
    assert!(app.tag_menu_shown());
    let _ = app.update(Message::Tags(crate::action_bar::Event::Toggle(
        "vr".to_owned(),
    )));
    assert!(app.grid_places().index(&slot(tag("vr"), "hades")).is_some());
    assert!(
        app.grid_places()
            .index(&slot(tag("rpg"), "hades"))
            .is_some()
    );
    let _ = app.update(Message::Tags(crate::action_bar::Event::CloseMenu));
    // Hide takes every copy of both, and their marks.
    let _ = app.update(Message::Tags(crate::action_bar::Event::ToggleFlag(
        crate::library_actions::Flag::Hidden,
    )));
    assert_eq!(app.grid_places().ids, [slot(tag("rpg"), "zelda")]);
    assert!(app.marked.is_empty());
    assert!(!app.action_bar_shown());
}

#[test]
fn a_grouped_cursor_falls_back_through_filters_regroups_rescans_and_removals() {
    let cards = || {
        vec![
            played(tagged("hades", &["rpg", "co-op"]), 5),
            tagged("celeste", &["platformer"]),
            tagged("zelda", &["rpg"]),
        ]
    };
    let mut app = grouped(cards(), GroupBy::Tag);
    let coop = slot(tag("co-op"), "hades");
    click_slot(&mut app, &coop, Modifiers::empty());
    assert_eq!(app.selected_slot(), Some(coop.clone()));

    // A filter that hides another game keeps the cursor's copy.
    app.filters.not_tags.insert("platformer".to_owned());
    let _ = app.resorted();
    assert_eq!(app.selected_slot(), Some(coop.clone()));
    // One that hides the game leaves nothing to point at, and the panel
    // stays; clearing it comes back to the same copy.
    app.search = "zel".to_owned();
    let _ = app.resorted();
    assert_eq!(app.selected_slot(), None);
    assert!(app.panel_open);
    app.search.clear();
    app.filters.not_tags.clear();
    let _ = app.resorted();
    assert_eq!(app.selected_slot(), Some(coop.clone()));

    // A regroup without that section falls back to the game's first copy.
    app.group = GroupBy::Source;
    let _ = app.resorted();
    let first = app.selected_slot().unwrap();
    assert_eq!(first.name(), "hades");
    assert_eq!(first.section(), &SectionKey::Recent);
    app.group = GroupBy::Tag;
    let _ = app.resorted();
    assert_eq!(app.selected_slot().unwrap().name(), "hades");

    // A rescan that drops hades from Recently played, with the cursor
    // there, falls back to its first copy left.
    click_slot(
        &mut app,
        &slot(SectionKey::Recent, "hades"),
        Modifiers::empty(),
    );
    let mut unplayed = cards();
    unplayed[0].last_played_at = None;
    app.merge(unplayed);
    assert_eq!(app.selected_slot(), Some(coop.clone()));
    assert!(app.panel_open);

    // A removal of another game keeps the cursor; removing hades clears
    // it and closes the panel.
    click_slot(&mut app, &slot(tag("rpg"), "hades"), Modifiers::empty());
    let mut fewer = cards();
    fewer[0].last_played_at = None;
    fewer.remove(1);
    app.merge(fewer.clone());
    assert_eq!(app.selected_slot(), Some(slot(tag("rpg"), "hades")));
    fewer.remove(0);
    app.merge(fewer);
    assert_eq!(app.selected, None);
    assert_eq!(app.selected_slot(), None);
    assert!(!app.panel_open);
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
    let next = walk(places, at.map(Spot::Card), direction).unwrap();
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
                        walk(&places, current.map(Spot::Card), direction),
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

/// Recently played holds hades, rpg holds hades and celeste, and No tag
/// holds notepad: the plan's question 22 library.
fn collapsible() -> App {
    let mut app = grouped(
        vec![
            played(tagged("hades", &["rpg"]), 5),
            tagged("celeste", &["rpg"]),
            tagged("notepad", &[]),
        ],
        GroupBy::Tag,
    );
    let _ = settle_grid(&mut app);
    app
}

fn header_keys(app: &App) -> Vec<(SectionKey, usize, bool)> {
    app.grid_places()
        .headers
        .iter()
        .map(|place| {
            (
                place.header.key.clone(),
                place.header.count,
                place.header.collapsed,
            )
        })
        .collect()
}

fn ctrl_a(app: &mut App) {
    let _ = app.update(Message::KeyWithFocus(
        Key::Character("a".into()),
        Modifiers::CTRL,
        iced::event::Status::Ignored,
        false,
    ));
}

#[test]
fn a_collapsed_section_keeps_its_header_and_count_and_opens_again() {
    // Plan 0062, question 13's A.
    let mut app = collapsible();
    let notepad_before = app.grid_places().slots[3];
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    assert_eq!(
        header_keys(&app),
        [
            (SectionKey::Recent, 1, false),
            (tag("rpg"), 2, true),
            (SectionKey::NoTag, 1, false),
        ]
    );
    let places = app.grid_places();
    assert_eq!(
        places.ids,
        [
            slot(SectionKey::Recent, "hades"),
            slot(SectionKey::NoTag, "notepad")
        ]
    );
    // What's below moves up, springing there.
    assert!(app.grid_motion.active());
    assert!(places.slots[1].y < notepad_before.y);
    // Nothing about it is saved.
    assert!(app.view_writes.group.is_none());
    assert!(
        !app.paths.library_file().exists() || {
            let saved = std::fs::read_to_string(app.paths.library_file()).unwrap();
            !saved.contains("collapsed")
        }
    );
    // Collapsing lasts the session, through a regroup and back.
    app.group = GroupBy::Source;
    app.reorder();
    app.group = GroupBy::Tag;
    app.reorder();
    assert_eq!(header_keys(&app)[1], (tag("rpg"), 2, true));
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    assert_eq!(header_keys(&app)[1], (tag("rpg"), 2, false));
    assert_eq!(app.grid_places().ids.len(), 4);
}

#[test]
fn ctrl_a_marks_only_what_is_in_view() {
    // Plan 0062, question 22's A: hades comes in through Recently played.
    let mut app = collapsible();
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    ctrl_a(&mut app);
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades", "notepad"]);
}

#[test]
fn collapsing_drops_only_the_marks_with_no_copy_left_in_view() {
    // Plan 0062, question 22's A.
    let mut app = collapsible();
    ctrl_a(&mut app);
    assert_eq!(app.marked.len(), 3);
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    // hades is still in Recently played; celeste had only rpg.
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades", "notepad"]);
    // Collapsing hades's other copy too drops its mark.
    let _ = app.update(Message::ToggleSection(SectionKey::Recent));
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["notepad"]);
    // Opening a section never brings a mark back.
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    assert_eq!(app.marked.len(), 1);
    // A mark can't land on a game with no copy in view.
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    app.toggle_mark("celeste");
    assert!(!app.marked.contains("celeste"));
}

#[test]
fn a_range_skips_a_collapsed_section() {
    // Plan 0062, question 22's A: Shift+click across rpg skips it.
    let mut app = collapsible();
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    let _ = app.update(Message::Press(slot(SectionKey::Recent, "hades")));
    app.modifiers = Modifiers::SHIFT;
    let _ = app.update(Message::Press(slot(SectionKey::NoTag, "notepad")));
    app.modifiers = Modifiers::empty();
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades", "notepad"]);
}

#[test]
fn a_range_starts_at_the_copy_that_was_ctrl_clicked() {
    let mut app = grouped(
        vec![
            tagged("hades", &["co-op", "rpg"]),
            tagged("celeste", &["rpg"]),
            tagged("zelda", &["rpg"]),
        ],
        GroupBy::Tag,
    );
    let _ = settle_grid(&mut app);
    let ids = app.grid_places().ids.clone();
    assert_eq!(
        ids,
        [
            slot(tag("co-op"), "hades"),
            slot(tag("rpg"), "celeste"),
            slot(tag("rpg"), "hades"),
            slot(tag("rpg"), "zelda"),
        ]
    );
    app.modifiers = Modifiers::CTRL;
    let _ = app.update(Message::Press(slot(tag("rpg"), "hades")));
    app.modifiers = Modifiers::SHIFT;
    let _ = app.update(Message::Press(slot(tag("rpg"), "zelda")));
    app.modifiers = Modifiers::empty();
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades", "zelda"]);
}

#[test]
fn the_random_picker_draws_by_game_from_what_is_in_view() {
    // Plan 0062, question 22's A, and a game shown twice isn't twice as
    // likely.
    let mut app = collapsible();
    app.random_pool = wineo_core::library::RandomPool::Any;
    let mut rng = fastrand::Rng::with_seed(7);
    let mut seen = BTreeSet::new();
    for _ in 0..200 {
        seen.insert(app.random_choice(&mut rng).unwrap());
    }
    assert_eq!(seen.len(), 3);
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    let mut seen = BTreeSet::new();
    let mut hades = 0;
    for _ in 0..400 {
        let pick = app.random_choice(&mut rng).unwrap();
        hades += usize::from(pick == "hades");
        seen.insert(pick);
    }
    assert_eq!(
        seen,
        BTreeSet::from(["hades".to_owned(), "notepad".to_owned()])
    );
    // Two games, one pick each: hades's one copy in view is one chance.
    assert!((120..280).contains(&hades), "{hades}");
    // With every section collapsed there's nothing to pick.
    for key in [SectionKey::Recent, SectionKey::NoTag] {
        let _ = app.update(Message::ToggleSection(key));
    }
    assert!(app.grid_places().ids.is_empty());
    assert_eq!(app.random_choice(&mut rng), None);
}

#[test]
fn the_cursors_section_collapsing_moves_it_to_the_header_and_keeps_the_panel() {
    // Plan 0062, question 22.
    let mut app = collapsible();
    let celeste = slot(tag("rpg"), "celeste");
    let _ = app.update(Message::Press(celeste));
    assert!(app.panel_open);
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    assert_eq!(app.header_cursor, Some(tag("rpg")));
    assert_eq!(app.active_slot, None);
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("celeste"));
    // The selected game has no copy in view, so keys that act on it wait.
    assert!(!app.selected_shown());
    let key = |app: &mut App, named: Named| {
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(named),
            Modifiers::empty(),
            iced::event::Status::Ignored,
            false,
        ));
    };
    // Down from the header goes to the first card after it.
    key(&mut app, Named::ArrowDown);
    assert_eq!(
        app.selected_slot(),
        Some(slot(SectionKey::NoTag, "notepad"))
    );
    assert_eq!(app.header_cursor, None);
    // Back on the header, Up goes to the row above and Left to the card
    // before it in shown order.
    app.header_cursor = Some(tag("rpg"));
    key(&mut app, Named::ArrowUp);
    assert_eq!(app.selected_slot(), Some(slot(SectionKey::Recent, "hades")));
    app.header_cursor = Some(tag("rpg"));
    key(&mut app, Named::ArrowLeft);
    assert_eq!(app.selected_slot(), Some(slot(SectionKey::Recent, "hades")));
    app.header_cursor = Some(tag("rpg"));
    key(&mut app, Named::ArrowRight);
    assert_eq!(
        app.selected_slot(),
        Some(slot(SectionKey::NoTag, "notepad"))
    );
    // A cursor outside the section stays where it is.
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    let _ = app.update(Message::ToggleSection(SectionKey::Recent));
    assert_eq!(app.header_cursor, None);
    assert_eq!(
        app.selected_slot(),
        Some(slot(SectionKey::NoTag, "notepad"))
    );
}

#[test]
fn a_cursor_on_the_last_header_with_nothing_below_stays_there() {
    let mut app = collapsible();
    let _ = app.update(Message::Press(slot(SectionKey::NoTag, "notepad")));
    let _ = app.update(Message::ToggleSection(SectionKey::NoTag));
    assert_eq!(app.header_cursor, Some(SectionKey::NoTag));
    let _ = app.update(Message::KeyWithFocus(
        Key::Named(Named::ArrowDown),
        Modifiers::empty(),
        iced::event::Status::Ignored,
        false,
    ));
    assert_eq!(app.header_cursor, Some(SectionKey::NoTag));
    assert_eq!(app.selected.as_deref(), Some("notepad"));
    // The header going away, as on a regroup, takes the cursor with it.
    let _ = app.update(Message::Sort(Event::Group(GroupBy::None)));
    assert_eq!(app.header_cursor, None);
}

#[test]
fn headers_take_tab_focus_after_the_toolbar_and_toggle_on_enter() {
    use crate::focus_button::{focusables, press};
    let app = collapsible();
    let mut laid = crate::layout_probe::lay_out(app.view(), Size::new(1200.0, 800.0));
    let ids = focusables(&mut laid);
    let position = |id: iced::widget::Id| {
        let id = Some(format!("{id:?}"));
        ids.iter().position(|found| *found == id)
    };
    let sort = position(iced::widget::Id::new(crate::sort_control::SORT_BUTTON_ID)).unwrap();
    let headers: Vec<usize> = app
        .grid_places()
        .headers
        .iter()
        .map(|place| position(place.header.key.header_id()).expect("a focusable header"))
        .collect();
    assert_eq!(headers.len(), 3);
    assert!(headers.iter().all(|header| *header > sort), "{ids:?}");
    assert!(headers.windows(2).all(|pair| pair[0] < pair[1]));
    crate::focus_button::run_task(
        &mut laid,
        iced::widget::operation::focus::<Message>(tag("rpg").header_id()),
    );
    let (messages, status) = press(&mut laid, Named::Enter, Modifiers::empty());
    assert_eq!(status, iced::event::Status::Captured);
    assert!(
        matches!(messages.as_slice(), [Message::ToggleSection(key)] if *key == tag("rpg")),
        "{messages:?}"
    );
    let (messages, _) = press(&mut laid, Named::Space, Modifiers::empty());
    assert!(matches!(messages.as_slice(), [Message::ToggleSection(_)]));
}

#[test]
fn a_header_toggle_waits_while_a_dialog_covers_the_library() {
    let mut app = collapsible();
    app.close_prompt = true;
    let _ = app.update(Message::ToggleSection(tag("rpg")));
    assert!(app.collapsed.is_empty());
}

#[test]
fn every_card_operation_skips_headers() {
    // Headers are never in the card slots, so an index into them is always
    // a card: the zoom's and layout switch's anchor, the hide's next
    // card, the range, and art all read cards.
    let mut app = collapsible();
    app.scroll_offset = 0.0;
    let (anchor, _) = app.find_anchor();
    let (anchor, _) = anchor.expect("a card in view");
    assert_eq!(anchor, slot(SectionKey::Recent, "hades"));
    let places = app.grid_places();
    assert_eq!(places.ids.len(), places.slots.len());
    assert_eq!(app.slot_cards(&places).len(), places.ids.len());
    // A hide of the first card hands the cursor to the next card, not to
    // the rpg header between them.
    let _ = app.update(Message::Press(slot(SectionKey::Recent, "hades")));
    let place = app.place_after_leaving(|name| name == "hades");
    assert_eq!(place, Some(0));
    if let Some(card) = app.cards.iter_mut().find(|card| card.name == "hades") {
        card.hidden = true;
    }
    app.order.cards_edited();
    app.reorder();
    let _ = app.select_in_place(place.unwrap());
    assert_eq!(app.selected_slot(), Some(slot(tag("rpg"), "celeste")));
}

/// Eleven played games, a to k, played newest to oldest, grouped by Recently
/// played, so k is the one game All has that Recently played doesn't.
fn eleven_played() -> App {
    let cards = ('a'..='k')
        .enumerate()
        .map(|(age, name)| played(tagged(&name.to_string(), &[]), 100 - age as i64))
        .collect();
    let mut app = grouped(cards, GroupBy::Recent);
    let _ = settle_grid(&mut app);
    assert_eq!(app.grid_places().ids[9], slot(SectionKey::Recent, "j"));
    assert_eq!(app.grid_places().ids[10], slot(all(), "a"));
    app
}

#[test]
fn hiding_hands_the_cursor_to_the_next_slot_even_when_recent_fills_up() {
    // k comes into Recently played as a leaves it, which would put the
    // cursor on k there had it counted places.
    let mut app = eleven_played();
    let _ = app.update(Message::Press(slot(all(), "a")));
    let _ = app.update(Message::Mark(crate::library_actions::Event::Toggle(
        "a".to_owned(),
        crate::library_actions::Flag::Hidden,
    )));
    assert_eq!(app.grid_places().ids[9], slot(SectionKey::Recent, "k"));
    assert_eq!(app.selected_slot(), Some(slot(all(), "b")));
}

#[test]
fn a_bulk_hide_hands_the_cursor_to_the_next_slot_even_when_recent_fills_up() {
    let mut app = eleven_played();
    let _ = app.update(Message::Press(slot(all(), "a")));
    app.toggle_mark("a");
    app.toggle_mark("c");
    let _ = app.update(Message::Tags(crate::action_bar::Event::ToggleFlag(
        crate::library_actions::Flag::Hidden,
    )));
    assert_eq!(app.grid_places().ids[8], slot(SectionKey::Recent, "k"));
    assert_eq!(app.selected_slot(), Some(slot(all(), "b")));
}

/// Twelve played games, a to l, l played last, grouped by Recently played,
/// so a and b are the games All has that Recently played doesn't.
fn twelve_played() -> App {
    let cards = ('a'..='l')
        .enumerate()
        .map(|(age, name)| played(tagged(&name.to_string(), &[]), age as i64))
        .collect();
    let mut app = grouped(cards, GroupBy::Recent);
    let _ = settle_grid(&mut app);
    assert_eq!(app.grid_places().ids.last(), Some(&slot(all(), "l")));
    app
}

#[test]
fn hiding_the_last_slot_hands_the_cursor_back_even_when_recent_fills_up() {
    // a and b come into Recently played as k and l leave it, which would
    // put the cursor on i had it counted places from the end.
    let mut app = twelve_played();
    let _ = app.update(Message::Press(slot(all(), "l")));
    app.toggle_mark("k");
    app.toggle_mark("l");
    let _ = app.update(Message::Tags(crate::action_bar::Event::ToggleFlag(
        crate::library_actions::Flag::Hidden,
    )));
    assert_eq!(app.grid_places().ids[9], slot(SectionKey::Recent, "a"));
    assert_eq!(app.selected_slot(), Some(slot(all(), "j")));

    let mut app = twelve_played();
    let _ = app.update(Message::Press(slot(all(), "l")));
    let _ = app.update(Message::Mark(crate::library_actions::Event::Toggle(
        "l".to_owned(),
        crate::library_actions::Flag::Hidden,
    )));
    assert_eq!(app.selected_slot(), Some(slot(all(), "k")));
}

/// The library mid-switch from no grouping to Group by Tag in `layout`:
/// the cards spring from their flat places, across the headers that just
/// appeared at theirs.
fn mid_regroup(layout: ViewLayout) -> App {
    let cards: Vec<Card> = ["alpha", "beta", "gamma", "delta"]
        .iter()
        .map(|name| {
            let mut card = tagged(name, &["rpg", "co-op"]);
            // Favorites show their star whether hovered or not.
            card.favorite = true;
            card
        })
        .collect();
    let mut app = grouped(cards, GroupBy::None);
    app.layout = layout;
    app.snap_grid();
    let _ = settle_grid(&mut app);
    app.group = GroupBy::Tag;
    let _ = app.resorted();
    assert!(app.grid_motion.active());
    app
}

/// Where a card is drawn over a header now, and the card's slot.
fn overlaps(app: &App) -> Vec<(SlotId, Rectangle)> {
    let places = app.grid_places();
    let mut found = Vec::new();
    for place in &places.headers {
        for (index, id) in places.ids.iter().enumerate() {
            if app.grid_motion.opacity(id) < 1.0 {
                continue;
            }
            if let Some(both) = app
                .grid_motion
                .drawn(&places, index)
                .intersection(&place.slot)
                && both.width > 2.0
                && both.height > 2.0
            {
                found.push((id.clone(), both));
            }
        }
    }
    found
}

/// What a press and release at `at` on the drawn grid publish, after the
/// pointer moves there.
fn click(app: &App, at: iced::Point) -> Vec<Message> {
    let mut laid = crate::layout_probe::lay_out(app.grid(), Size::new(app.viewport_width, 4000.0));
    let cursor = iced::mouse::Cursor::Available(at);
    let left = iced::mouse::Button::Left;
    let mut messages = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: at }),
        cursor,
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
        cursor,
    ));
    messages.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
        cursor,
    ));
    messages
}

/// Points across `area`, 16 px apart, inside its edges.
fn points(area: Rectangle) -> Vec<iced::Point> {
    let mut points = Vec::new();
    let mut y = area.y + 2.0;
    while y < area.y + area.height - 1.0 {
        let mut x = area.x + 2.0;
        while x < area.x + area.width - 1.0 {
            points.push(iced::Point::new(x, y));
            x += 16.0;
        }
        y += 16.0;
    }
    points
}

#[test]
fn a_card_springing_over_a_header_takes_the_pointer_before_the_header() {
    // Events follow the drawing: the card on top hears a press first, so
    // it never falls through to the header beneath it.
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        let app = mid_regroup(layout);
        let overlaps = overlaps(&app);
        assert!(!overlaps.is_empty(), "{layout:?}: no card over a header");
        let (mut presses, mut stars, mut hovers) = (0, 0, 0);
        // A few overlaps are plenty; each point lays out the whole grid.
        for (slot, area) in overlaps.iter().take(2) {
            for at in points(*area) {
                let messages = click(&app, at);
                assert!(
                    !messages
                        .iter()
                        .any(|message| matches!(message, Message::ToggleSection(_))),
                    "{layout:?} at {at:?}: {messages:?}"
                );
                for message in &messages {
                    match message {
                        Message::Press(pressed) if pressed == slot => presses += 1,
                        Message::Mark(crate::library_actions::Event::PosterStar(starred))
                            if starred == slot =>
                        {
                            stars += 1;
                        }
                        Message::Hover(Some(hovered)) if hovered == slot => hovers += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!(presses > 0 && hovers > 0, "{layout:?}");
        assert!(
            stars > 0,
            "{layout:?}: the star over a header took no press"
        );
    }
}

#[test]
fn ctrl_and_shift_on_a_card_over_a_header_mark_and_never_collapse() {
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        for modifiers in [Modifiers::empty(), Modifiers::CTRL, Modifiers::SHIFT] {
            let mut app = mid_regroup(layout);
            let (slot, area) = overlaps(&app)[0].clone();
            let at = area.center();
            let messages = click(&app, at);
            app.modifiers = modifiers;
            for message in messages {
                let _ = app.update(message);
            }
            app.modifiers = Modifiers::empty();
            assert!(app.collapsed.is_empty(), "{layout:?} {modifiers:?}");
            if modifiers.is_empty() {
                assert_eq!(app.selected_slot(), Some(slot.clone()), "{layout:?}");
                assert!(app.marked.is_empty());
            } else {
                assert!(app.marked.contains(slot.name()), "{layout:?} {modifiers:?}");
            }
        }
    }
}

#[test]
fn a_press_on_a_header_with_no_card_over_it_collapses_its_section() {
    for layout in [ViewLayout::Grid, ViewLayout::List] {
        let mut app = mid_regroup(layout);
        let _ = settle_grid(&mut app);
        assert!(overlaps(&app).is_empty());
        let header = app.grid_places().headers[0].clone();
        let messages = click(&app, header.slot.center());
        assert!(
            matches!(messages.as_slice(), [Message::ToggleSection(key)] if *key == header.header.key),
            "{layout:?}: {messages:?}"
        );
        for message in messages {
            let _ = app.update(message);
        }
        assert!(app.collapsed.contains(&header.header.key), "{layout:?}");
    }
}

// Group by Recently played: the pinned sections around an All that is the
// flat library (plan 0062, "Step 10 checkpoint").

fn all() -> SectionKey {
    SectionKey::All
}

fn hidden_game(card: Card) -> Card {
    let mut card = card;
    card.hidden = true;
    card
}

#[test]
fn group_by_recent_menu_sits_second_after_none() {
    let labels: Vec<&str> = crate::sections::GROUPS
        .into_iter()
        .map(crate::sections::label)
        .collect();
    assert_eq!(
        labels,
        ["None", "Recently played", "Tag", "Source", "State"]
    );
}

#[test]
fn group_by_recent_is_recently_played_then_all_in_the_shown_order() {
    // hades is newest, so it leads Recently played, while All keeps the
    // sort's order, which is the flat library's.
    let cards = vec![
        played(tagged("hades", &["rpg"]), 20),
        played(tagged("celeste", &[]), 10),
        tagged("notepad", &[]),
    ];
    let sections = sections_of(
        &cards,
        GroupBy::Recent,
        Query::default(),
        &Filters::default(),
    );
    assert_eq!(
        sections,
        [
            (SectionKey::Recent, names(&["hades", "celeste"])),
            (all(), names(&["celeste", "hades", "notepad"])),
        ]
    );
    // A recent game is in both, as two copies.
    let both = sections
        .iter()
        .filter(|(_, games)| games.contains(&"hades".to_owned()))
        .count();
    assert_eq!(both, 2);
}

#[test]
fn all_under_recent_matches_group_by_none_for_every_sort() {
    let mut cards = vec![
        played(tagged("hades", &["rpg"]), 20),
        played(tagged("celeste", &[]), 10),
        tagged("notepad", &[]),
        tagged("anodyne", &["rpg"]),
    ];
    cards[1].favorite = true;
    cards[3].seconds = 90;
    for sort in SortKey::ALL {
        for descending in [false, true] {
            for favorites_first in [false, true] {
                let query = Query {
                    sort,
                    descending,
                    favorites_first,
                };
                let filters = Filters::default();
                let flat = sections_of(&cards, GroupBy::None, query, &filters);
                let recent = sections_of(&cards, GroupBy::Recent, query, &filters);
                let all_cards = &recent.iter().find(|(key, _)| *key == all()).unwrap().1;
                assert_eq!(
                    *all_cards, flat[0].1,
                    "{sort:?} {descending} {favorites_first}"
                );
            }
        }
    }
}

#[test]
fn recent_takes_the_ten_newest_and_all_still_holds_every_game() {
    let cards: Vec<Card> = (0..=RECENT)
        .map(|index| {
            played(
                tagged(&format!("game-{index:02}"), &[]),
                i64::try_from(index).unwrap(),
            )
        })
        .collect();
    let sections = sections_of(
        &cards,
        GroupBy::Recent,
        Query::default(),
        &Filters::default(),
    );
    assert_eq!(sections[0].0, SectionKey::Recent);
    assert_eq!(sections[0].1.len(), RECENT);
    assert!(!sections[0].1.contains(&"game-00".to_owned()));
    assert_eq!(sections[1].0, all());
    assert_eq!(sections[1].1.len(), RECENT + 1);
}

#[test]
fn recent_hides_hidden_games_until_show_hidden_and_then_puts_them_last_alone() {
    let cards = vec![
        played(tagged("hades", &[]), 1),
        hidden_game(played(tagged("secret", &[]), 99)),
        tagged("notepad", &[]),
    ];
    let query = Query::default();
    let off = sections_of(&cards, GroupBy::Recent, query, &Filters::default());
    assert_eq!(
        off,
        [
            (SectionKey::Recent, names(&["hades"])),
            (all(), names(&["hades", "notepad"])),
        ]
    );
    let on = Filters {
        show_hidden: true,
        ..Filters::default()
    };
    // The hidden game leaves All and stays out of Recently played, so
    // Hidden is the only place it shows, as under every other grouping.
    let sections = sections_of(&cards, GroupBy::Recent, query, &on);
    assert_eq!(
        sections,
        [
            (SectionKey::Recent, names(&["hades"])),
            (all(), names(&["hades", "notepad"])),
            (SectionKey::Hidden, names(&["secret"])),
        ]
    );
    // Only hidden games left: All is empty, so it doesn't show.
    let only = vec![hidden_game(tagged("secret", &[]))];
    let sections = sections_of(&only, GroupBy::Recent, query, &on);
    assert_eq!(sections, [(SectionKey::Hidden, names(&["secret"]))]);
}

#[test]
fn recent_filters_narrow_both_sections() {
    let mut vr = played(tagged("beat", &["vr"]), 30);
    vr.favorite = true;
    let cards = vec![played(tagged("hades", &["rpg"]), 20), vr];
    let filters = Filters {
        not_tags: BTreeSet::from(["vr".to_owned()]),
        ..Filters::default()
    };
    let sections = sections_of(&cards, GroupBy::Recent, Query::default(), &filters);
    assert_eq!(
        sections,
        [
            (SectionKey::Recent, names(&["hades"])),
            (all(), names(&["hades"])),
        ]
    );
}

#[test]
fn recent_with_nothing_played_shows_only_all_with_a_header() {
    let cards = vec![tagged("hades", &[]), tagged("notepad", &[])];
    let query = Query::default();
    let sections = sections_of(&cards, GroupBy::Recent, query, &Filters::default());
    assert_eq!(sections, [(all(), names(&["hades", "notepad"]))]);

    let app = grouped(cards.clone(), GroupBy::Recent);
    assert_eq!(header_keys(&app), [(all(), 2, false)]);
    assert_eq!(app.grid_places().ids.len(), 2);
    // Group by None has no header, over the same cards.
    let flat = grouped(cards, GroupBy::None);
    assert!(flat.grid_places().headers.is_empty());
    assert_eq!(flat.grid_places().ids, app.grid_places().ids);

    // It collapses like any section, header and count kept.
    let mut app = app;
    let _ = app.update(Message::ToggleSection(all()));
    assert_eq!(header_keys(&app), [(all(), 2, true)]);
    assert!(app.grid_places().ids.is_empty());
    let _ = app.update(Message::ToggleSection(all()));
    assert_eq!(app.grid_places().ids.len(), 2);
}

#[test]
fn recent_with_no_games_shows_nothing() {
    let sections = sections_of(&[], GroupBy::Recent, Query::default(), &Filters::default());
    assert_eq!(sections, []);
    let app = grouped(Vec::new(), GroupBy::Recent);
    assert!(app.grid_places().headers.is_empty());
    assert!(app.grid_places().ids.is_empty());
}

/// Recently played holds hades, and All holds celeste, hades, and notepad.
fn recent_library() -> App {
    let mut app = grouped(
        vec![
            played(tagged("hades", &["rpg"]), 5),
            tagged("celeste", &["rpg"]),
            tagged("notepad", &[]),
        ],
        GroupBy::Recent,
    );
    let _ = settle_grid(&mut app);
    app
}

#[test]
fn the_app_draws_both_recent_sections_under_headers() {
    let app = recent_library();
    assert_eq!(
        app.grid_places().ids,
        [
            slot(SectionKey::Recent, "hades"),
            slot(all(), "celeste"),
            slot(all(), "hades"),
            slot(all(), "notepad"),
        ]
    );
    assert_eq!(
        header_keys(&app),
        [(SectionKey::Recent, 1, false), (all(), 3, false)]
    );
    assert_eq!(app.slot_cards(&app.grid_places()).len(), 4);
}

#[test]
fn show_hidden_adds_hidden_last_under_recent() {
    let mut app = grouped(
        vec![
            played(tagged("hades", &[]), 5),
            hidden_game(tagged("secret", &[])),
        ],
        GroupBy::Recent,
    );
    app.filters.show_hidden = true;
    app.reorder();
    assert_eq!(
        header_keys(&app),
        [
            (SectionKey::Recent, 1, false),
            (all(), 1, false),
            (SectionKey::Hidden, 1, false),
        ]
    );
    assert_eq!(
        app.grid_places().ids.last(),
        Some(&slot(SectionKey::Hidden, "secret"))
    );
}

#[test]
fn a_game_joins_recently_played_under_recent_when_its_session_ends() {
    let mut app = grouped(
        vec![tagged("hades", &[]), tagged("celeste", &[])],
        GroupBy::Recent,
    );
    assert_eq!(header_keys(&app), [(all(), 2, false)]);
    app.merge(vec![
        played(tagged("hades", &[]), 5),
        tagged("celeste", &[]),
    ]);
    assert_eq!(
        app.grid_places().ids,
        [
            slot(SectionKey::Recent, "hades"),
            slot(all(), "celeste"),
            slot(all(), "hades"),
        ]
    );
}

#[test]
fn collapsing_all_marks_and_picks_only_through_recently_played() {
    // Question 22's A: with All collapsed, hades is in view through
    // Recently played and nothing else is.
    let mut app = recent_library();
    let _ = app.update(Message::ToggleSection(all()));
    assert_eq!(
        header_keys(&app),
        [(SectionKey::Recent, 1, false), (all(), 3, true)]
    );
    assert_eq!(app.grid_places().ids, [slot(SectionKey::Recent, "hades")]);
    ctrl_a(&mut app);
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades"]);

    // Opening All and marking everything, then collapsing it, keeps the
    // mark with a copy left in view.
    let _ = app.update(Message::ToggleSection(all()));
    ctrl_a(&mut app);
    assert_eq!(app.marked.len(), 3);
    let _ = app.update(Message::ToggleSection(all()));
    let marked: Vec<&str> = app.marked.iter().map(String::as_str).collect();
    assert_eq!(marked, ["hades"]);

    // With both collapsed there is nothing to pick.
    let _ = app.update(Message::ToggleSection(SectionKey::Recent));
    assert!(app.marked.is_empty());
    app.random_pool = wineo_core::library::RandomPool::Any;
    assert_eq!(app.random_choice(&mut fastrand::Rng::with_seed(3)), None);
}

#[test]
fn all_with_nothing_recent_collapses_and_ctrl_a_marks_nothing() {
    let mut app = grouped(
        vec![tagged("hades", &[]), tagged("celeste", &[])],
        GroupBy::Recent,
    );
    let _ = app.update(Message::ToggleSection(all()));
    ctrl_a(&mut app);
    assert!(app.marked.is_empty());
}

#[test]
fn a_collapsed_all_is_forgotten_by_none_and_remembered_by_recent() {
    let mut app = recent_library();
    let _ = app.update(Message::ToggleSection(all()));
    let _ = app.update(Message::Sort(Event::Group(GroupBy::None)));
    // The flat grid has no header to collapse, so every game shows.
    assert!(app.grid_places().headers.is_empty());
    assert_eq!(app.grid_places().ids.len(), 3);
    let _ = app.update(Message::Sort(Event::Group(GroupBy::Recent)));
    assert_eq!(header_keys(&app)[1], (all(), 3, true));
}

#[test]
fn the_cursor_in_all_moves_to_its_header_when_it_collapses() {
    let mut app = recent_library();
    let _ = app.update(Message::Press(slot(all(), "celeste")));
    assert!(app.panel_open);
    let _ = app.update(Message::ToggleSection(all()));
    assert_eq!(app.header_cursor, Some(all()));
    assert_eq!(app.active_slot, None);
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("celeste"));
    let key = |app: &mut App, named: Named| {
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(named),
            Modifiers::empty(),
            iced::event::Status::Ignored,
            false,
        ));
    };
    // Nothing is below it, so Down stays.
    key(&mut app, Named::ArrowDown);
    assert_eq!(app.header_cursor, Some(all()));
    // Up goes to the row above, which is Recently played.
    key(&mut app, Named::ArrowUp);
    assert_eq!(app.selected_slot(), Some(slot(SectionKey::Recent, "hades")));
    assert_eq!(app.header_cursor, None);
}

#[test]
fn arrow_keys_walk_from_recently_played_into_all_by_slot() {
    let mut app = recent_library();
    let _ = app.update(Message::Press(slot(SectionKey::Recent, "hades")));
    let key = |app: &mut App, named: Named| {
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(named),
            Modifiers::empty(),
            iced::event::Status::Ignored,
            false,
        ));
    };
    // Right follows the shown order, onto All's first card.
    key(&mut app, Named::ArrowRight);
    assert_eq!(app.selected_slot(), Some(slot(all(), "celeste")));
    key(&mut app, Named::ArrowRight);
    // The second copy of hades moves the cursor and not the panel.
    assert_eq!(app.selected_slot(), Some(slot(all(), "hades")));
}

#[test]
fn switching_between_none_and_recent_springs_all_and_fades_the_extra_copy() {
    // Question 17's pairing: All is the flat library's section, so each of
    // its cards pairs with its flat slot, and only the recent copy fades.
    let mut app = grouped(
        vec![
            played(tagged("hades", &[]), 5),
            tagged("celeste", &[]),
            tagged("notepad", &[]),
        ],
        GroupBy::None,
    );
    let _ = settle_grid(&mut app);
    let flat = app.grid_places();
    let before: Vec<iced::Point> = flat
        .ids
        .iter()
        .enumerate()
        .map(|(index, _)| app.grid_motion.drawn(&flat, index).position())
        .collect();
    app.group = GroupBy::Recent;
    let _ = app.resorted();
    assert!(app.grid_motion.active());
    let places = app.grid_places();
    for (id, from) in flat.ids.iter().zip(&before) {
        let moved = slot(all(), id.name());
        assert_eq!(app.grid_motion.opacity(&moved), 1.0, "{id:?}");
        let index = places.index(&moved).unwrap();
        assert_eq!(app.grid_motion.drawn(&places, index).position(), *from);
    }
    assert_eq!(
        app.grid_motion.opacity(&slot(SectionKey::Recent, "hades")),
        0.0
    );
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
    assert_eq!(leaving, [&slot(SectionKey::Recent, "hades")]);
    for name in ["celeste", "hades", "notepad"] {
        assert_eq!(app.grid_motion.opacity(&slot(all(), name)), 1.0, "{name}");
    }
}

#[test]
fn recent_headers_take_tab_focus_and_all_toggles_on_enter() {
    use crate::focus_button::{focusables, press};
    let app = recent_library();
    let mut laid = crate::layout_probe::lay_out(app.view(), Size::new(1200.0, 800.0));
    let ids = focusables(&mut laid);
    let position = |id: iced::widget::Id| {
        let id = Some(format!("{id:?}"));
        ids.iter().position(|found| *found == id)
    };
    let sort = position(iced::widget::Id::new(crate::sort_control::SORT_BUTTON_ID)).unwrap();
    let recent = position(SectionKey::Recent.header_id()).expect("a focusable header");
    let all_header = position(all().header_id()).expect("a focusable header");
    assert!(sort < recent && recent < all_header, "{ids:?}");
    crate::focus_button::run_task(
        &mut laid,
        iced::widget::operation::focus::<Message>(all().header_id()),
    );
    let (messages, status) = press(&mut laid, Named::Enter, Modifiers::empty());
    assert_eq!(status, iced::event::Status::Captured);
    assert!(
        matches!(messages.as_slice(), [Message::ToggleSection(key)] if *key == all()),
        "{messages:?}"
    );

    // Group by None has no All header to reach.
    let flat = grouped(vec![tagged("hades", &[])], GroupBy::None);
    let mut laid = crate::layout_probe::lay_out(flat.view(), Size::new(1200.0, 800.0));
    let ids = focusables(&mut laid);
    let header = Some(format!("{:?}", all().header_id()));
    assert!(!ids.contains(&header));
}

#[test]
fn group_by_recent_is_saved_and_reopens_grouped_that_way() {
    let (_root, mut app) = fixture();
    crate::sort_control::tests::open_sort(&mut app);
    let _ = app.update(Message::Sort(Event::OpenGroups));
    let _ = app.update(Message::Sort(Event::Group(GroupBy::Recent)));
    assert_eq!(app.group, GroupBy::Recent);
    // Nothing played, so All alone, under a header.
    assert_eq!(app.grid_places().headers.len(), 1);
    finish_group_write(&mut app);
    let saved = std::fs::read_to_string(app.paths.library_file()).unwrap();
    assert!(saved.contains("group = \"recent\""), "{saved}");
    let reopened = App::boot(app.paths.clone()).0;
    assert_eq!(reopened.group, GroupBy::Recent);
}

#[test]
fn only_grouping_by_state_reads_state_so_recent_keeps_the_rescan_shortcut() {
    let idle = card("hades", State::Inactive, 0);
    let mut running = idle.clone();
    running.state = State::Running;
    assert!(order::same_inputs(&idle, &running, GroupBy::Recent));
    // A new last played time still counts, since it changes the section.
    let mut played_now = idle.clone();
    played_now.last_played_at = at(5);
    assert!(!order::same_inputs(&idle, &played_now, GroupBy::Recent));
}
