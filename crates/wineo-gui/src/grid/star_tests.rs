//! The poster star: shown on hover and on a favorite, a click that toggles
//! the favorite and nothing else, and the slot it keeps for itself.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::advanced::widget::{Id, Operation};
use iced::mouse::{self, Cursor};
use iced::{Point, Rectangle, Size, Vector};
use wineo_core::runtime::State;

use super::poster_tests::{
    art_card, card_node, decoded, gog, gog_at, layers, mark_chip, overlay_shown,
};
use super::*;
use crate::layout_probe::{Laid, bounds_at, lay_out, relay_out, render};
use crate::library_actions::STAR_TARGET;

/// An app on its own throwaway HOME with `names` loaded, the ones in
/// `favorites` starred and the ones in `hidden` hidden.
fn fixture(names: &[&str], favorites: &[&str], hidden: &[&str]) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards = names
        .iter()
        .map(|name| {
            let mut card = crate::tests::card(name, State::Inactive, 60);
            card.favorite = favorites.contains(name);
            card.hidden = hidden.contains(name);
            card
        })
        .collect();
    let _ = app.update(Message::Loaded(Ok(cards)));
    (root, app)
}

fn grid_size(app: &App) -> Size {
    Size::new(app.viewport_width, f32::INFINITY)
}

fn index(app: &App, name: &str) -> usize {
    app.filtered()
        .iter()
        .position(|card| card.name == name)
        .expect(name)
}

/// The star's center on a card laid out at `card`.
fn star_on(app: &App, card: Rectangle) -> Point {
    let art = card.position() + Vector::new(2.0, 2.0);
    let width = app.grid_layout().art_frame().width;
    Point::new(art.x + width - 8.0 - 12.0, art.y + 8.0 + 12.0)
}

fn moved(at: Point) -> iced::Event {
    iced::Event::Mouse(mouse::Event::CursorMoved { position: at })
}

fn pressed(button: mouse::Button) -> iced::Event {
    iced::Event::Mouse(mouse::Event::ButtonPressed(button))
}

fn released(button: mouse::Button) -> iced::Event {
    iced::Event::Mouse(mouse::Event::ButtonReleased(button))
}

/// A left click at `at`: what the press and the release published.
fn click<M>(laid: &mut Laid<'_, M>, at: Point) -> Vec<M> {
    let mut messages = laid.event_at(&pressed(mouse::Button::Left), Cursor::Available(at));
    messages.extend(laid.event_at(&released(mouse::Button::Left), Cursor::Available(at)));
    messages
}

fn toggles(messages: &[Message], name: &str) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                Message::Mark(Event::Toggle(found, Flag::Favorite)) if found == name
            )
        })
        .count()
}

fn favorite(app: &App, name: &str) -> bool {
    app.cards
        .iter()
        .find(|card| card.name == name)
        .unwrap()
        .favorite
}

/// The bounds of the card containers with these names, by name.
fn cards_in_view(app: &App, names: &[&str]) -> Vec<Rectangle> {
    struct Found(Vec<Id>, Arc<Mutex<Vec<(Id, Rectangle)>>>);
    impl Operation for Found {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
            if let Some(id) = id
                && self.0.contains(id)
            {
                self.1.lock().unwrap().push((id.clone(), bounds));
            }
        }
    }
    let ids: Vec<Id> = names
        .iter()
        .map(|name| Id::from(format!("library-card:{name}")))
        .collect();
    let found = Arc::default();
    let mut laid = lay_out(app.view(), app.window_size);
    laid.run(Box::new(Found(ids.clone(), Arc::clone(&found))));
    let found = found.lock().unwrap();
    ids.iter()
        .map(|id| {
            found
                .iter()
                .find(|(found, _)| found == id)
                .map(|(_, bounds)| *bounds)
                .expect("the card in view")
        })
        .collect()
}

/// Every focusable id in tree order, which is Tab's order.
fn focus_order(laid: &mut Laid<'_, Message>) -> Vec<String> {
    struct Order(Arc<Mutex<Vec<String>>>);
    impl Operation for Order {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&Id>,
            bounds: Rectangle,
            _state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            self.0.lock().unwrap().push(format!("{id:?} at {bounds:?}"));
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(Order(found.clone())));
    found.lock().unwrap().clone()
}

#[test]
fn a_star_click_toggles_the_favorite_and_never_selects_opens_or_launches() {
    let names = ["game-00", "game-01", "game-02"];
    let (_root, mut app) = fixture(&names, &[], &[]);
    let size = grid_size(&app);
    let mut laid = lay_out(app.grid(), size);
    let card = bounds_at(&laid.node, &[index(&app, "game-01")]);
    let messages = laid.event_at(&moved(card.center()), Cursor::Available(card.center()));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(Some(name)) if name == "game-01")),
        "{messages:?}"
    );
    let tree = laid.into_tree();
    for message in messages {
        let _ = app.update(message);
    }
    // The pointer moves onto the star that hovering showed: the card stays
    // hovered, with no exit and enter in between.
    let mut laid = relay_out(app.grid(), tree, size);
    let star = star_on(&app, card);
    let messages = laid.event_at(&moved(star), Cursor::Available(star));
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(app.hovered.as_deref(), Some("game-01"));
    let messages = click(&mut laid, star);
    assert_eq!(toggles(&messages, "game-01"), 1, "{messages:?}");
    assert_eq!(messages.len(), 1, "{messages:?}");
    let tree = laid.into_tree();
    for message in messages {
        let _ = app.update(message);
    }
    assert!(favorite(&app, "game-01"));
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
    assert!(app.operation.is_none());
    // A second click right after is no double-click: the filled star
    // unfavorites.
    let mut laid = relay_out(app.grid(), tree, size);
    let messages = click(&mut laid, star);
    assert_eq!(toggles(&messages, "game-01"), 1, "{messages:?}");
    assert_eq!(messages.len(), 1, "{messages:?}");
    let tree = laid.into_tree();
    for message in messages {
        let _ = app.update(message);
    }
    assert!(!favorite(&app, "game-01"));
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
    assert!(app.operation.is_none());
    // Leaving the card is still its exit.
    let mut laid = relay_out(app.grid(), tree, size);
    let outside = Point::new(card.x - 4.0, card.y - 4.0);
    let messages = laid.event_at(&moved(outside), Cursor::Available(outside));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Hover(None))),
        "{messages:?}"
    );
}

#[test]
fn a_right_click_on_the_star_still_opens_the_context_menu() {
    let (_root, mut app) = fixture(&["game"], &["game"], &[]);
    let mut laid = lay_out(app.grid(), grid_size(&app));
    let star = star_on(&app, bounds_at(&laid.node, &[0]));
    let mut messages = laid.event_at(&pressed(mouse::Button::Right), Cursor::Available(star));
    messages.extend(laid.event_at(&released(mouse::Button::Right), Cursor::Available(star)));
    // The first event over the card is also its enter.
    assert!(
        matches!(
            messages.as_slice(),
            [Message::Hover(Some(hovered)), Message::Context(name)]
                if hovered == "game" && name == "game"
        ),
        "{messages:?}"
    );
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    assert_eq!(app.action_menu.as_deref(), Some("game"));
    assert!(favorite(&app, "game"));
}

#[test]
fn hover_and_star_presses_do_nothing_under_a_flow_or_dialog() {
    let (_root, mut app) = fixture(&["game", "other"], &["game"], &[]);
    let [card] = cards_in_view(&app, &["game"])[..] else {
        unreachable!()
    };
    let star = star_on(&app, card);
    // Open, the same press reaches the star.
    let mut laid = lay_out(app.view(), app.window_size);
    assert_eq!(toggles(&click(&mut laid, star), "game"), 1);
    drop(laid);
    let opened: [fn(&mut App); 3] = [
        |app| app.flow = Some(crate::flow::Page::choose(app.paths.clone())),
        |app| app.close_prompt = true,
        |app| app.hook_stop_confirm = Some("game".into()),
    ];
    for open in opened {
        open(&mut app);
        let _ = app.update(Message::Hover(Some("other".into())));
        assert_eq!(app.hovered, None);
        let mut laid = lay_out(app.view(), app.window_size);
        let messages = click(&mut laid, star);
        assert_eq!(toggles(&messages, "game"), 0, "{messages:?}");
        drop(laid);
        // Nor does the message itself change anything.
        let _ = app.update(Message::Mark(Event::Toggle("game".into(), Flag::Favorite)));
        assert!(favorite(&app, "game"));
        app.flow = None;
        app.close_prompt = false;
        app.hook_stop_confirm = None;
    }
}

#[test]
fn the_star_fades_with_its_card_and_a_leaving_card_takes_no_presses() {
    let mut app = crate::tests::app();
    let card = crate::tests::card("game", State::Inactive, 60);
    app.hovered = Some(card.name.clone());
    let layout = app.grid_layout();
    let size = Size::new(200.0, 400.0);
    // The star's chip darkens the tile's corner, less so as it fades.
    let star = |pixels: &[u8]| {
        let (x, y) = (2 + 156 - 8 - 12, 2 + 8 + 12);
        pixels[(y * 200 + x) * 4..][..4].to_vec()
    };
    app.hovered = None;
    let plain = render(
        &mut app.card_drawn(&card, 1.0, layout, Details::Inline),
        size,
    );
    app.hovered = Some(card.name.clone());
    let shown = render(
        &mut app.card_drawn(&card, 1.0, layout, Details::Inline),
        size,
    );
    assert_ne!(star(&shown), star(&plain));
    let gone = render(
        &mut app.card_drawn(&card, 0.0, layout, Details::Inline),
        size,
    );
    assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));

    let (_root, mut app) = fixture(&["drop-00", "keep-00"], &["drop-00"], &[]);
    let mut laid = lay_out(app.grid(), grid_size(&app));
    let star = star_on(&app, bounds_at(&laid.node, &[index(&app, "drop-00")]));
    assert_eq!(toggles(&click(&mut laid, star), "drop-00"), 1);
    drop(laid);
    let _ = app.update(Message::SearchChanged("keep".into()));
    let leaving = app
        .grid_motion
        .leaving()
        .iter()
        .find(|card| card.name == "drop-00")
        .expect("the leaving card")
        .bounds;
    assert_eq!(app.hovered, None);
    let mut laid = lay_out(app.grid(), grid_size(&app));
    assert_eq!(star_on(&app, leaving), star);
    let messages = click(&mut laid, star);
    assert_eq!(toggles(&messages, "drop-00"), 0, "{messages:?}");
}

#[test]
fn the_gog_corner_is_the_same_at_rest_and_on_hover() {
    let mut app = crate::tests::app();
    for (poster_only, zoom) in [
        (true, 60),
        (true, 100),
        (false, 60),
        (false, 80),
        (false, 100),
    ] {
        app.poster_only = poster_only;
        app.zoom = zoom;
        for state in [State::Running, State::Inactive] {
            let mut card = crate::tests::card("game", state, 60);
            gog(&mut card);
            let chip = state == State::Running && app.grid_layout().text().chip();
            app.hovered = None;
            let rest = gog_at(&card_node(&app, &card), chip);
            app.hovered = Some(card.name.clone());
            let node = card_node(&app, &card);
            let hovered = gog_at(&node, chip);
            let what = format!("poster-only {poster_only} at {zoom}%, {state:?}");
            assert_eq!(rest, hovered, "{what}");
            // And the hover star's chip keeps clear of it.
            let star = mark_chip(&node, 0);
            let gap = if hovered.1 { 0.0 } else { 4.0 };
            assert!(
                hovered.0.x + hovered.0.width <= star.x - gap + 0.01,
                "{what}"
            );
        }
    }
}

#[test]
fn a_hidden_cards_eye_keeps_the_second_slot() {
    let mut app = crate::tests::app();
    let mut card = crate::tests::card("game", State::Inactive, 60);
    card.hidden = true;
    let eye = |app: &App, card: &Card| {
        let node = card_node(app, card);
        let art = bounds_at(&node, &[0, 0]);
        mark_chip(&node, 1).position() - art.position()
    };
    let width = app.grid_layout().art_frame().width;
    let place = Vector::new(width - 8.0 - 24.0, 8.0 + 24.0 + 4.0);
    // At rest, with nothing in the star's slot.
    assert_eq!(eye(&app, &card), place);
    // Hovered, with the outline star above it.
    app.hovered = Some(card.name.clone());
    assert_eq!(eye(&app, &card), place);
    // A favorite, hovered and at rest.
    card.favorite = true;
    assert_eq!(eye(&app, &card), place);
    app.hovered = None;
    assert_eq!(eye(&app, &card), place);
    // The star sits in the top slot, the eye under it.
    let node = card_node(&app, &card);
    let art = bounds_at(&node, &[0, 0]);
    assert_eq!(
        mark_chip(&node, 0).position() - art.position(),
        Vector::new(width - 8.0 - 24.0, 8.0)
    );
    let star = bounds_at(&node, &[0, 0, 0, layers(&node).len() - 1, 0, 0]);
    assert_eq!(star.size(), Size::new(STAR_TARGET, STAR_TARGET));
    assert_eq!(
        bounds_at(&node, &[0, 0, 0, layers(&node).len() - 1, 0, 0, 0]),
        mark_chip(&node, 0)
    );
}

#[test]
fn a_star_click_that_resorts_moves_the_card_and_its_eye_stays_put() {
    let names = ["game-00", "game-01", "game-02", "game-03"];
    let (_root, mut app) = fixture(&names, &[], &["game-03"]);
    let _ = app.update(Message::Filter(
        crate::filter_control::Event::ToggleShowHidden,
    ));
    let _ = app.update(Message::Sort(
        crate::sort_control::Event::ToggleFavoritesFirst,
    ));
    assert!(app.query.favorites_first);
    let _ = crate::tests::settle_grid(&mut app);
    let before = index(&app, "game-03");
    assert_ne!(before, 0);
    let card = app.filtered()[before].clone();
    let eye = |app: &App| {
        let node = card_node(app, &card_of(app, "game-03"));
        let art = bounds_at(&node, &[0, 0]);
        mark_chip(&node, 1).position() - art.position()
    };
    let rest = eye(&app);
    let _ = app.update(Message::Hover(Some(card.name.clone())));
    assert_eq!(eye(&app), rest);
    let mut laid = lay_out(app.grid(), grid_size(&app));
    let star = star_on(&app, bounds_at(&laid.node, &[before]));
    let messages = click(&mut laid, star);
    assert_eq!(toggles(&messages, "game-03"), 1, "{messages:?}");
    drop(laid);
    for message in messages {
        let _ = app.update(message);
    }
    // Favorites first: it leads now and springs away from the pointer.
    assert!(favorite(&app, "game-03"));
    assert_eq!(index(&app, "game-03"), 0);
    assert!(app.grid_motion.active());
    assert_eq!(eye(&app), rest);
    let _ = crate::tests::settle_grid(&mut app);
    let _ = app.update(Message::Hover(None));
    assert_eq!(eye(&app), rest);
    assert!(app.selected.is_none());
}

fn card_of(app: &App, name: &str) -> Card {
    app.cards
        .iter()
        .find(|card| card.name == name)
        .unwrap()
        .clone()
}

#[test]
fn the_star_shows_in_poster_only_above_the_overlay() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    let card = art_card("game", State::Inactive);
    decoded(&mut app, &card, [128, 128, 128]);
    app.hovered = Some(card.name.clone());
    let node = card_node(&app, &card);
    assert!(overlay_shown(&node));
    // The poster, the overlay, then the marks on top.
    assert_eq!(layers(&node).len(), 3);
    let art = bounds_at(&node, &[0, 0]);
    assert_eq!(
        mark_chip(&node, 0).position() - art.position(),
        Vector::new(art.width - 8.0 - 24.0, 8.0)
    );
    // A click on it reaches the star through the overlay.
    let mut laid = lay_out(app.card(&card), Size::new(400.0, 900.0));
    let messages = click(&mut laid, mark_chip(&node, 0).center());
    assert_eq!(toggles(&messages, "game"), 1, "{messages:?}");
    assert_eq!(messages.len(), 1, "{messages:?}");
    drop(laid);
    // Not hovered or a favorite, it shows no star.
    app.hovered = None;
    app.selected = Some(card.name.clone());
    let node = card_node(&app, &card);
    assert!(overlay_shown(&node));
    assert_eq!(layers(&node).len(), 2);
}

#[test]
fn star_presses_are_ignored_while_cards_are_displaced_from_their_slots() {
    let names = ["game-00", "game-01", "game-02"];
    let (_root, mut app) = fixture(&names, &["game-01"], &[]);
    let at = index(&app, "game-01");
    let size = grid_size(&app);
    let mut laid = lay_out(app.grid(), size);
    let star = star_on(&app, bounds_at(&laid.node, &[at]));
    // In its slot, the press reaches the star.
    assert_eq!(toggles(&click(&mut laid, star), "game-01"), 1);
    drop(laid);
    for _ in 0..10 {
        let _ = app.update(Message::SectionClicked(crate::sidebar::Section::Library));
    }
    assert!(!app.collapse.idle());
    let tick = |app: &mut App, after: Duration| {
        let generation = app.collapse.generation();
        let _ = app.update(Message::CollapseTick(generation, Instant::now() + after));
    };
    // While the shelves shake the cards are still drawn in place, and
    // after they've dropped they're drawn loose.
    for after in [Duration::ZERO, Duration::from_secs(4)] {
        tick(&mut app, after);
        let mut laid = lay_out(app.grid(), size);
        let loose = bounds_at(&laid.node, &[at]);
        for point in [star, star_on(&app, loose)] {
            let messages = click(&mut laid, point);
            assert_eq!(toggles(&messages, "game-01"), 0, "{messages:?}");
        }
        assert!(favorite(&app, "game-01"));
    }
    assert!(app.collapse.resting());
}

#[test]
fn the_star_is_no_tab_stop() {
    let (_root, mut app) = fixture(&["game-00", "game-01"], &["game-00"], &[]);
    let order = |app: &App| focus_order(&mut lay_out(app.view(), app.window_size));
    let _ = app.update(Message::Hover(None));
    let rest = order(&app);
    let _ = app.update(Message::Hover(Some("game-01".into())));
    assert_eq!(app.hovered.as_deref(), Some("game-01"));
    assert_eq!(order(&app), rest);
    let _ = app.update(Message::Mark(Event::Toggle(
        "game-01".into(),
        Flag::Favorite,
    )));
    assert!(favorite(&app, "game-01"));
    assert_eq!(order(&app), rest);
}
