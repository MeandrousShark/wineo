//! One game drawn in two slots, which only tests can make until the
//! library groups: each copy keeps its own place, motion, hover, star pop,
//! widget id, and widget state.

use std::time::{Duration, Instant};

use iced::advanced::widget::{Id, Operation};
use iced::mouse::{self, Cursor};
use iced::{Element, Point, Rectangle, Size, Vector};
use wineo_core::runtime::State;

use super::{GridCard, GridLayout, GridMotion, Places, ViewLayout};
use crate::grid::Details;
use crate::hover_fade::HoverFades;
use crate::layout_probe::{bounds_at, lay_out, relay_out};
use crate::slot::{SectionKey, SlotId};
use crate::star_pop::StarPops;
use crate::tests::card;
use crate::{App, Message};

fn flat(name: &str) -> SlotId {
    SlotId::new(SectionKey::All, name)
}

fn other(name: &str) -> SlotId {
    SlotId::new(SectionKey::Test("rpg"), name)
}

/// `ids` measured as a row of grid cards.
fn places(ids: &[SlotId], columns: usize) -> Places {
    let layout = GridLayout::default();
    Places::new(
        ids.to_vec(),
        &vec![layout.poster_height() + 60.0; ids.len()],
        layout.card_width(),
        columns,
        0.0,
        layout,
        ViewLayout::Grid,
    )
}

#[test]
fn each_copy_of_a_game_has_its_own_slot() {
    let ids = [flat("hades"), other("hades"), flat("celeste")];
    let measured = places(&ids, 3);
    assert_eq!(measured.ids, ids);
    assert_eq!(measured.index(&flat("hades")), Some(0));
    assert_eq!(measured.index(&other("hades")), Some(1));
    assert_eq!(measured.index(&other("celeste")), None);
    // The first copy in shown order stands for the game.
    assert_eq!(measured.first("hades"), Some(0));
    assert_eq!(measured.first("celeste"), Some(2));
    assert_eq!(measured.first("absent"), None);
    let rpg_first = places(&[other("hades"), flat("hades")], 3);
    assert_eq!(rpg_first.first("hades"), Some(0));
    assert_eq!(rpg_first.index(&flat("hades")), Some(1));
}

#[test]
fn two_copies_spring_and_leave_on_their_own() {
    let start = Instant::now();
    let mut motion = GridMotion::default();
    let before = places(&[flat("hades"), other("hades"), flat("celeste")], 3);
    motion.snap_to(&before);
    // The copies trade places, so each springs from its own old slot.
    let swapped = places(&[other("hades"), flat("hades"), flat("celeste")], 3);
    motion.spring_to(&swapped, start);
    assert!(motion.active());
    let drawn = motion.shown(&swapped);
    assert_eq!(drawn[0].position(), before.slots[1].position());
    assert_eq!(drawn[1].position(), before.slots[0].position());
    assert_eq!(
        motion.offset(&other("hades")),
        -motion.offset(&flat("hades"))
    );
    assert_ne!(motion.offset(&flat("hades")), Vector::ZERO);
    // One copy goes: only it fades out, where it was drawn.
    let one = places(&[flat("hades"), flat("celeste")], 3);
    motion.spring_to(&one, start + Duration::from_millis(16));
    let leaving: Vec<_> = motion.leaving().iter().map(|card| &card.id).collect();
    assert_eq!(leaving, [&other("hades")]);
    assert_eq!(motion.opacity(&flat("hades")), 1.0);
    // A zoom's anchor is a slot too.
    motion.anchor = Some((flat("hades"), 10.0));
    motion.snap_to(&one);
    assert!(motion.leaving().is_empty());
    assert_eq!(motion.anchor, None);
}

#[test]
fn a_zoom_with_a_copy_swapped_for_another_springs_instead_of_scaling() {
    let mut motion = GridMotion::default();
    motion.snap_to(&places(&[flat("hades"), flat("celeste")], 3));
    let generation = motion.generation();
    // Same count, same game names, but a different slot.
    let moved = places(&[other("hades"), flat("celeste")], 3);
    motion.zoom_to(&moved, Instant::now());
    // A spring, which only scaling wouldn't start: the new copy takes the
    // body hades left (plan 0062, question 17), so it neither fades in nor
    // leaves one fading out.
    assert_ne!(motion.generation(), generation);
    assert!(motion.bodies.contains_key(&other("hades")));
    assert!(!motion.bodies.contains_key(&flat("hades")));
    assert_eq!(motion.opacity(&other("hades")), 1.0);
    assert!(motion.leaving().is_empty());
}

#[test]
fn hover_and_glow_belong_to_one_copy_and_the_selection_to_every_copy() {
    let ids = [flat("hades"), other("hades"), flat("celeste")];
    let mut fades = HoverFades::default();
    fades.sync(Some(&other("hades")), None, Some(&other("hades")), &ids);
    assert!(fades.fading(&other("hades")));
    assert!(!fades.fading(&flat("hades")));
    assert_eq!(fades.hovered(), Some(&other("hades")));
    let now = Instant::now();
    let generation = fades.generation();
    fades.tick(generation, now);
    fades.tick(generation, now + crate::grid_motion::FADE);
    assert!(!fades.active());
    assert_eq!(fades.progress(&other("hades"), true, false), (1.0, 1.0));
    assert_eq!(fades.glow(&other("hades"), true), 1.0);

    // Selecting the game fades in the overlay on both its copies.
    let mut fades = HoverFades::default();
    assert!(fades.selection_moves(Some("hades")));
    fades.sync(None, Some("hades"), None, &ids);
    assert!(!fades.selection_moves(Some("hades")));
    assert!(fades.fading(&flat("hades")));
    assert!(fades.fading(&other("hades")));
    assert!(!fades.fading(&flat("celeste")));
    // Dropping the slots no longer shown keeps the rest.
    fades.retain(|slot| *slot != other("hades"));
    assert!(fades.fading(&flat("hades")));
    assert!(!fades.fading(&other("hades")));
}

#[test]
fn a_star_pops_only_on_the_copy_clicked() {
    let mut pops = StarPops::default();
    pops.start(&other("hades"));
    assert_eq!(pops.progress(&other("hades")), Some(0.0));
    assert_eq!(pops.progress(&flat("hades")), None);
    pops.retain(|slot| *slot == flat("hades"));
    assert!(!pops.active());
}

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards = vec![card("hades", State::Inactive, 60)];
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

/// The grid with `app`'s one game drawn in each of `ids`, as the app
/// draws its cards.
fn grid<'a>(app: &'a App, motion: &GridMotion, ids: &[SlotId]) -> Element<'a, Message> {
    let layout = app.grid_layout();
    let card = &app.cards[0];
    let cards = ids
        .iter()
        .map(|id| GridCard {
            id: id.clone(),
            card: app.card_drawn(id, card, 1.0, layout, Details::Apart),
            details: None,
        })
        .collect();
    super::grid(
        cards,
        Vec::new(),
        motion,
        false,
        None,
        layout,
        ViewLayout::Grid,
        0.0,
    )
}

fn moved(at: Point) -> iced::Event {
    iced::Event::Mouse(mouse::Event::CursorMoved { position: at })
}

fn size(app: &App) -> Size {
    Size::new(app.viewport_width, f32::INFINITY)
}

#[test]
fn each_copy_answers_for_its_own_slot() {
    let (_root, app) = fixture();
    let ids = [flat("hades"), other("hades")];
    let motion = GridMotion::default();
    type Found = std::sync::Arc<std::sync::Mutex<Vec<Id>>>;
    struct Ids(Found);
    impl Operation for Ids {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&Id>, _bounds: Rectangle) {
            self.0.lock().unwrap().extend(id.cloned());
        }
    }
    let found = Found::default();
    let mut laid = lay_out(grid(&app, &motion, &ids), size(&app));
    laid.run(Box::new(Ids(found.clone())));
    let found = found.lock().unwrap().clone();
    // The flat library's id is the one cards always had.
    assert!(found.contains(&Id::from("library-card:hades".to_owned())));
    assert!(found.contains(&other("hades").widget_id()));
    // The pointer on the second copy hovers that copy alone.
    let at = bounds_at(&laid.node, &[1]).center();
    let messages = laid.event_at(&moved(at), Cursor::Available(at));
    let hovered: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Hover(Some(slot)) => Some(slot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(hovered, [other("hades")], "{messages:?}");
}

#[test]
fn widget_state_follows_the_slot_when_copies_trade_places() {
    let (_root, app) = fixture();
    let motion = GridMotion::default();
    let mut laid = lay_out(
        grid(&app, &motion, &[flat("hades"), other("hades")]),
        size(&app),
    );
    let at = bounds_at(&laid.node, &[0]).center();
    let messages = laid.event_at(&moved(at), Cursor::Available(at));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(Some(slot)) if *slot == flat("hades"))),
        "{messages:?}"
    );
    let tree = laid.into_tree();
    // Keyed by game, the two copies' hover state would mix up here.
    let mut laid = relay_out(
        grid(&app, &motion, &[other("hades"), flat("hades")]),
        tree,
        size(&app),
    );
    let messages = laid.event_at(&moved(at), Cursor::Available(at));
    assert!(
        messages.iter().any(
            |message| matches!(message, Message::Hover(Some(slot)) if *slot == other("hades"))
        ),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Unhover(slot) if *slot == flat("hades"))),
        "{messages:?}"
    );
}

#[test]
fn a_hovered_copy_draws_hovered_and_the_other_doesnt() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Hover(Some(other("hades"))));
    assert_eq!(app.hovered, Some(other("hades")));
    let card = app.cards[0].clone();
    let layout = app.grid_layout();
    let pixels = |slot: SlotId| {
        let mut view = app.card_drawn(&slot, &card, 1.0, layout, Details::Inline);
        crate::layout_probe::render(&mut view, Size::new(400.0, 900.0))
    };
    assert_ne!(pixels(other("hades")), pixels(flat("hades")));
    // Leaving the other copy doesn't end this one's hover.
    let _ = app.update(Message::Unhover(flat("hades")));
    assert_eq!(app.hovered, Some(other("hades")));
    let _ = app.update(Message::Unhover(other("hades")));
    assert_eq!(app.hovered, None);
}
