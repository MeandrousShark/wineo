//! The hover fades: the poster star following the hover and the poster-only
//! overlay following the hover or selection, over the cards' 150 ms fade.

use std::time::{Duration, Instant};

use iced::advanced::subscription::into_recipes;
use iced::mouse::{self, Cursor};
use iced::{Point, Size};
use wineo_core::runtime::State;

use super::poster_tests::{art_card, card_node, decoded, mark_chip};
use super::*;
use crate::layout_probe::{lay_out, render};

/// An app on its own throwaway HOME with `names` loaded as art cards, the
/// ones in `favorites` starred, each art decoded in white.
fn fixture(names: &[&str], favorites: &[&str]) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let mut app = App::boot(paths).0;
    let cards: Vec<Card> = names
        .iter()
        .map(|name| {
            let mut card = art_card(name, State::Inactive);
            card.favorite = favorites.contains(name);
            card
        })
        .collect();
    for card in &cards {
        decoded(&mut app, card, [255, 255, 255]);
    }
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert!(!app.hover_fades.active());
    (root, app)
}

/// The test's frame clock, which the fades read from.
struct Clock(Instant);

impl Clock {
    /// Starts at the first frame after an update that set something fading.
    fn start(app: &mut App) -> Self {
        let now = Instant::now();
        let _ = app.update(Message::HoverTick(app.hover_fades.generation(), now));
        Self(now)
    }

    fn tick(&mut self, app: &mut App, millis: u64) {
        self.0 += Duration::from_millis(millis);
        let _ = app.update(Message::HoverTick(app.hover_fades.generation(), self.0));
    }
}

fn hover(app: &mut App, name: Option<&str>) {
    let _ = app.update(Message::Hover(name.map(Into::into)));
}

fn card_of(app: &App, name: &str) -> Card {
    app.cards
        .iter()
        .find(|card| card.name == name)
        .unwrap()
        .clone()
}

/// The star's and the overlay's strength on `name` as the grid draws it.
fn progress(app: &App, name: &str) -> (f32, f32) {
    let hovered = app.hovered.as_deref() == Some(name);
    let selected = app.selected.as_deref() == Some(name);
    app.hover_fades.progress(name, hovered, selected)
}

fn star(app: &App, name: &str) -> f32 {
    progress(app, name).0
}

fn overlay(app: &App, name: &str) -> f32 {
    progress(app, name).1
}

fn near(value: f32, expected: f32) -> bool {
    (value - expected).abs() < 0.03
}

fn frames(app: &App) -> usize {
    into_recipes(app.hover_fades.subscription()).len()
}

/// At rest: nothing stored and no frames asked for.
fn settled(app: &App) -> bool {
    !app.hover_fades.active() && frames(app) == 0
}

fn size() -> Size {
    Size::new(200.0, 300.0)
}

/// The star chip's backing beside its icon, darker as the star shows.
fn star_shade(app: &App, card: &Card) -> u8 {
    let pixels = render(&mut app.card(card), size());
    let (x, y) = (2 + 156 - 8 - 12 - 9, 2 + 8 + 12);
    pixels[(y * 200 + x) * 4]
}

/// The overlay's gradient below its text, darker as the overlay shows.
fn overlay_shade(app: &App, card: &Card) -> u8 {
    let pixels = render(&mut app.card(card), size());
    let (x, y) = (80, 235);
    pixels[(y * 200 + x) * 4]
}

fn left_click(app: &App, card: &Card, at: Point) -> Vec<Message> {
    let mut laid = lay_out(app.card(card), Size::new(400.0, 900.0));
    let mut messages = laid.event_at(
        &iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Cursor::Available(at),
    );
    messages.extend(laid.event_at(
        &iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        Cursor::Available(at),
    ));
    messages
}

/// The favorite toggles for `name`, from the poster star or anywhere else.
fn toggles(messages: &[Message], name: &str) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                Message::Mark(Event::Toggle(found, Flag::Favorite) | Event::PosterStar(found))
                    if found == name
            )
        })
        .count()
}

#[test]
fn the_star_fades_in_after_hover_starts_and_out_after_it_ends() {
    let (_root, mut app) = fixture(&["game", "other"], &[]);
    let card = card_of(&app, "game");
    let plain = star_shade(&app, &card);
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    assert!(near(star(&app, "game"), 0.0));
    assert!(frames(&app) > 0);
    clock.tick(&mut app, 75);
    assert!(near(star(&app, "game"), 0.5), "{}", star(&app, "game"));
    let half = star_shade(&app, &card);
    clock.tick(&mut app, 80);
    assert_eq!(star(&app, "game"), 1.0);
    assert!(settled(&app));
    let full = star_shade(&app, &card);
    assert!(full < half && half < plain, "{full} {half} {plain}");

    hover(&mut app, None);
    let mut clock = Clock::start(&mut app);
    assert!(near(star(&app, "game"), 1.0));
    clock.tick(&mut app, 75);
    assert!(near(star(&app, "game"), 0.5), "{}", star(&app, "game"));
    assert!(frames(&app) > 0);
    clock.tick(&mut app, 80);
    assert_eq!(star(&app, "game"), 0.0);
    assert!(settled(&app));
    assert_eq!(star_shade(&app, &card), plain);
}

#[test]
fn a_reversal_mid_fade_carries_on_from_where_the_star_is() {
    let (_root, mut app) = fixture(&["game"], &[]);
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 90);
    let at = star(&app, "game");
    assert!(near(at, 0.6), "{at}");
    hover(&mut app, None);
    assert_eq!(star(&app, "game"), at);
    clock.tick(&mut app, 30);
    assert!(near(star(&app, "game"), 0.4), "{}", star(&app, "game"));
    let at = star(&app, "game");
    hover(&mut app, Some("game"));
    assert_eq!(star(&app, "game"), at);
    clock.tick(&mut app, 45);
    assert!(near(star(&app, "game"), 0.7), "{}", star(&app, "game"));
    clock.tick(&mut app, 60);
    assert!(settled(&app));
    assert_eq!(star(&app, "game"), 1.0);
}

#[test]
fn a_selected_card_keeps_its_overlay_after_the_hover_ends() {
    let (_root, mut app) = fixture(&["game", "other"], &[]);
    app.poster_only = true;
    let card = card_of(&app, "game");
    let _ = app.update(Message::Select("game".into()));
    let _ = crate::tests::settle_grid(&mut app);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    assert!(settled(&app));
    let full = overlay_shade(&app, &card);
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    hover(&mut app, None);
    let mut clock = Clock::start(&mut app);
    for _ in 0..4 {
        clock.tick(&mut app, 40);
        assert_eq!(overlay(&app, "game"), 1.0);
        assert_eq!(overlay_shade(&app, &card), full);
    }
    assert_eq!(star(&app, "game"), 0.0);
    assert!(settled(&app));

    // Selecting another card fades this overlay out and that one in.
    let other = card_of(&app, "other");
    let _ = app.update(Message::Select("other".into()));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(near(overlay(&app, "game"), 0.5));
    assert!(near(overlay(&app, "other"), 0.5));
    let half = overlay_shade(&app, &card);
    assert!(half > full, "{half} {full}");
    assert_eq!(overlay_shade(&app, &other), half);
    clock.tick(&mut app, 80);
    assert!(settled(&app));
    assert_eq!(overlay_shade(&app, &other), full);
}

#[test]
fn a_quick_pass_across_three_cards_leaves_each_fading_from_where_it_was() {
    let (_root, mut app) = fixture(&["a", "b", "c"], &[]);
    hover(&mut app, Some("a"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 100);
    let a = star(&app, "a");
    assert!(near(a, 0.667), "{a}");
    hover(&mut app, Some("b"));
    assert_eq!(star(&app, "a"), a);
    assert_eq!(star(&app, "b"), 0.0);
    clock.tick(&mut app, 50);
    assert!(near(star(&app, "a"), 0.333));
    assert!(near(star(&app, "b"), 0.333));
    hover(&mut app, Some("c"));
    clock.tick(&mut app, 25);
    assert!(near(star(&app, "a"), 0.167));
    assert!(near(star(&app, "b"), 0.167));
    assert!(near(star(&app, "c"), 0.167));
    clock.tick(&mut app, 25);
    assert!(near(star(&app, "a"), 0.0));
    assert!(near(star(&app, "b"), 0.0));
    assert!(near(star(&app, "c"), 0.333));
    clock.tick(&mut app, 200);
    assert!(settled(&app));
    assert_eq!(
        (star(&app, "a"), star(&app, "b"), star(&app, "c")),
        (0.0, 0.0, 1.0)
    );
}

#[test]
fn a_card_filtered_out_or_removed_mid_fade_drops_its_fade() {
    let (_root, mut app) = fixture(&["drop", "keep"], &[]);
    app.poster_only = true;
    hover(&mut app, Some("drop"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(app.hover_fades.fading("drop"));
    let _ = app.update(Message::SearchChanged("keep".into()));
    assert_eq!(app.hovered, None);
    assert!(!app.hover_fades.fading("drop"));
    assert!(settled(&app));
    // The leaving card draws neither the star nor the overlay.
    assert!(
        app.grid_motion
            .leaving()
            .iter()
            .any(|card| card.name == "drop")
    );
    assert_eq!(progress(&app, "drop"), (0.0, 0.0));
    let node = card_node(&app, &card_of(&app, "drop"));
    assert_eq!(super::poster_tests::layers(&node).len(), 1);
    assert!(!super::poster_tests::overlay_shown(&node));

    let _ = app.update(Message::SearchChanged(String::new()));
    let _ = crate::tests::settle_grid(&mut app);
    hover(&mut app, Some("keep"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(app.hover_fades.fading("keep"));
    let remaining: Vec<Card> = app
        .cards
        .iter()
        .filter(|card| card.name != "keep")
        .cloned()
        .collect();
    let _ = app.update(Message::Loaded(Ok(remaining)));
    assert!(!app.hover_fades.fading("keep"));
    assert!(settled(&app));
}

#[test]
fn a_flow_or_dialog_opening_mid_fade_lets_it_finish() {
    let (_root, mut app) = fixture(&["game", "other"], &[]);
    let opened: [fn(&mut App); 3] = [
        |app| app.flow = Some(crate::flow::Page::choose(app.paths.clone())),
        |app| app.close_prompt = true,
        |app| app.hook_stop_confirm = Some("game".into()),
    ];
    let close = |app: &mut App| {
        app.flow = None;
        app.close_prompt = false;
        app.hook_stop_confirm = None;
    };
    for open in opened {
        hover(&mut app, Some("game"));
        let mut clock = Clock::start(&mut app);
        clock.tick(&mut app, 75);
        let at = star(&app, "game");
        open(&mut app);
        // Hover is ignored under it, and the fade carries on regardless.
        hover(&mut app, Some("other"));
        assert_eq!(app.hovered.as_deref(), Some("game"));
        assert_eq!(star(&app, "game"), at);
        assert!(frames(&app) > 0);
        let during = into_recipes(app.subscription()).len();
        clock.tick(&mut app, 80);
        assert_eq!(star(&app, "game"), 1.0);
        assert!(settled(&app));
        assert_eq!(into_recipes(app.subscription()).len(), during - 1);
        close(&mut app);

        // Fading out, it finishes under the overlay too.
        hover(&mut app, None);
        let mut clock = Clock::start(&mut app);
        clock.tick(&mut app, 75);
        let at = star(&app, "game");
        open(&mut app);
        hover(&mut app, Some("game"));
        assert_eq!(app.hovered, None);
        assert_eq!(star(&app, "game"), at);
        clock.tick(&mut app, 80);
        assert_eq!(star(&app, "game"), 0.0);
        assert!(settled(&app));
        close(&mut app);
    }
}

#[test]
fn leaving_the_library_mid_fade_still_lets_it_finish() {
    let (_root, mut app) = fixture(&["game"], &[]);
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    let at = star(&app, "game");
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    assert_eq!(app.hovered, None);
    assert_eq!(star(&app, "game"), at);
    clock.tick(&mut app, 40);
    assert!(star(&app, "game") < at);
    clock.tick(&mut app, 80);
    assert!(settled(&app));
}

#[test]
fn a_favorites_filled_star_ignores_the_fade() {
    let (_root, mut app) = fixture(&["fav", "plain"], &["fav"]);
    let card = card_of(&app, "fav");
    let rest = star_shade(&app, &card);
    hover(&mut app, Some("fav"));
    assert_eq!(star(&app, "fav"), 1.0);
    hover(&mut app, None);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(near(star(&app, "fav"), 0.5));
    assert_eq!(star_shade(&app, &card), rest);
    clock.tick(&mut app, 80);
    assert!(settled(&app));
    assert_eq!(star_shade(&app, &card), rest);
}

#[test]
fn unfavoriting_a_hovered_favorite_shows_the_outline_star_at_full_strength() {
    let (_root, mut app) = fixture(&["fav", "plain"], &["fav"]);
    let plain = card_of(&app, "plain");
    app.hovered = Some("plain".into());
    let full = star_shade(&app, &plain);
    app.hovered = None;
    // Straight after the pointer arrives, before any frame.
    hover(&mut app, Some("fav"));
    assert_eq!(star(&app, "fav"), 1.0);
    let _ = app.update(Message::Mark(Event::Toggle("fav".into(), Flag::Favorite)));
    assert!(!card_of(&app, "fav").favorite);
    assert_eq!(star(&app, "fav"), 1.0);
    assert_eq!(star_shade(&app, &card_of(&app, "fav")), full);

    // Favorited mid-fade-in and unfavorited again, it's full too.
    hover(&mut app, Some("plain"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 30);
    assert!(star(&app, "plain") < 1.0);
    let _ = app.update(Message::Mark(Event::Toggle("plain".into(), Flag::Favorite)));
    let _ = app.update(Message::Mark(Event::Toggle("plain".into(), Flag::Favorite)));
    assert!(!card_of(&app, "plain").favorite);
    assert_eq!(star(&app, "plain"), 1.0);
    clock.tick(&mut app, 200);
    assert!(settled(&app));
}

#[test]
fn the_star_takes_presses_fading_in_and_none_fading_out() {
    let (_root, mut app) = fixture(&["game"], &[]);
    let card = card_of(&app, "game");
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 30);
    let at = mark_chip(&card_node(&app, &card)).center();
    let messages = left_click(&app, &card, at);
    assert_eq!(toggles(&messages, "game"), 1, "{messages:?}");
    clock.tick(&mut app, 200);
    hover(&mut app, None);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 30);
    assert!(star(&app, "game") > 0.0);
    // Still drawn, in the same place, but the press is the card's.
    assert_eq!(mark_chip(&card_node(&app, &card)).center(), at);
    let messages = left_click(&app, &card, at);
    assert_eq!(toggles(&messages, "game"), 0, "{messages:?}");
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Select(name) if name == "game")),
        "{messages:?}"
    );
}

#[test]
fn the_fading_overlay_is_no_pointer_target() {
    let (_root, mut app) = fixture(&["game"], &[]);
    app.poster_only = true;
    let card = card_of(&app, "game");
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(near(overlay(&app, "game"), 0.5));
    let node = card_node(&app, &card);
    assert!(super::poster_tests::overlay_shown(&node));
    let art = crate::layout_probe::bounds_at(&node, &[0, 0]);
    let at = Point::new(art.x + 20.0, art.y + art.height - 12.0);
    let messages = left_click(&app, &card, at);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Select(name) if name == "game")),
        "{messages:?}"
    );
}

/// The pointer onto `name`'s star, or off it.
fn star_hover(app: &mut App, name: &str, on: bool) {
    let _ = app.update(Message::StarHover(name.into(), on));
}

fn glow(app: &App, name: &str) -> f32 {
    app.hover_fades.glow(name, app.glowing_star() == Some(name))
}

/// The filled star's icon at the chip's center: white at rest, the primary
/// color lit up.
fn star_icon(app: &App, card: &Card) -> [u8; 4] {
    let pixels = render(&mut app.card(card), size());
    let (x, y) = (2 + 156 - 8 - 12, 2 + 8 + 12);
    pixels[(y * 200 + x) * 4..][..4].try_into().unwrap()
}

#[test]
fn the_star_glows_with_the_pointer_on_it_and_fades_back_after() {
    let (_root, mut app) = fixture(&["game", "other"], &[]);
    let card = card_of(&app, "game");
    hover(&mut app, Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    assert!(settled(&app));
    let hovered = star_shade(&app, &card);
    // The pointer on the card alone doesn't light it.
    assert_eq!(glow(&app, "game"), 0.0);

    star_hover(&mut app, "game", true);
    let mut clock = Clock::start(&mut app);
    assert!(near(glow(&app, "game"), 0.0));
    assert!(frames(&app) > 0);
    clock.tick(&mut app, 75);
    assert!(near(glow(&app, "game"), 0.5), "{}", glow(&app, "game"));
    let half = star_shade(&app, &card);
    clock.tick(&mut app, 80);
    assert_eq!(glow(&app, "game"), 1.0);
    assert!(settled(&app));
    let lit = star_shade(&app, &card);
    assert!(lit < half && half < hovered, "{lit} {half} {hovered}");
    // The outline star keeps its strength throughout.
    assert_eq!(star(&app, "game"), 1.0);

    star_hover(&mut app, "game", false);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    assert!(near(glow(&app, "game"), 0.5), "{}", glow(&app, "game"));
    clock.tick(&mut app, 80);
    assert!(settled(&app));
    assert_eq!(star_shade(&app, &card), hovered);
}

#[test]
fn a_favorites_filled_star_tints_to_the_primary_color() {
    let (_root, mut app) = fixture(&["fav"], &["fav"]);
    let card = card_of(&app, "fav");
    let rest = star_icon(&app, &card);
    assert!(rest[0] > 200 && rest[1] > 200 && rest[2] > 200, "{rest:?}");
    hover(&mut app, Some("fav"));
    star_hover(&mut app, "fav", true);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 75);
    let half = star_icon(&app, &card);
    clock.tick(&mut app, 80);
    assert!(settled(&app));
    let lit = star_icon(&app, &card);
    let primary = app.palette.primary.into_rgba8();
    for channel in 0..3 {
        assert!(
            lit[channel].abs_diff(primary[channel]) <= 8,
            "{lit:?} {primary:?}"
        );
    }
    // Halfway there it's between the two, and the card's fade still
    // applies on top.
    assert!(
        lit[0] < half[0] && half[0] < rest[0],
        "{lit:?} {half:?} {rest:?}"
    );
    let layout = app.grid_layout();
    let gone = render(
        &mut app.card_drawn(&card, 0.0, layout, Details::Inline),
        size(),
    );
    assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));
}

#[test]
fn a_star_that_takes_no_presses_never_glows() {
    let (_root, mut app) = fixture(&["game", "other"], &[]);
    // The star's enter can arrive just before its card's.
    star_hover(&mut app, "game", true);
    assert_eq!(glow(&app, "game"), 0.0);
    hover(&mut app, Some("game"));
    assert_eq!(app.glowing_star(), Some("game"));
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    assert!(settled(&app));

    // Fading out with the hover, it fades its glow too.
    hover(&mut app, None);
    assert_eq!(app.glowing_star(), None);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    assert!(settled(&app));
    assert_eq!(glow(&app, "game"), 0.0);

    // A hover the app moves away isn't the pointer coming back to the star.
    hover(&mut app, Some("game"));
    star_hover(&mut app, "game", true);
    let _ = app.update(Message::SearchChanged("other".into()));
    assert_eq!(app.hovered, None);
    let _ = app.update(Message::SearchChanged(String::new()));
    let _ = crate::tests::settle_grid(&mut app);
    hover(&mut app, Some("game"));
    assert_eq!(app.glowing_star(), None);
    let mut clock = Clock::start(&mut app);
    clock.tick(&mut app, 200);
    assert!(settled(&app));

    // Under a flow or dialog it fades out, and the pointer can't light it.
    let opened: [fn(&mut App); 3] = [
        |app| app.flow = Some(crate::flow::Page::choose(app.paths.clone())),
        |app| app.close_prompt = true,
        |app| app.hook_stop_confirm = Some("game".into()),
    ];
    for open in opened {
        star_hover(&mut app, "game", true);
        let mut clock = Clock::start(&mut app);
        clock.tick(&mut app, 200);
        assert_eq!(glow(&app, "game"), 1.0);
        open(&mut app);
        star_hover(&mut app, "game", false);
        assert_eq!(app.glowing_star(), None);
        let mut clock = Clock::start(&mut app);
        clock.tick(&mut app, 200);
        assert!(settled(&app));
        assert_eq!(glow(&app, "game"), 0.0);
        app.flow = None;
        app.close_prompt = false;
        app.hook_stop_confirm = None;
        // Uncovered with the card still hovered, it stays dark until the
        // pointer enters the star again.
        hover(&mut app, Some("game"));
        assert_eq!(app.glowing_star(), None);
        assert!(settled(&app));
    }
}
