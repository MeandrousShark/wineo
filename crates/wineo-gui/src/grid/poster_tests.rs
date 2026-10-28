//! Cards in the poster-only view and with the compact text below 90%: the
//! title overlay, the status chip beside the GOG pill, and their heights.

use iced::advanced::Layout;
use iced::advanced::layout::Node;
use iced::widget::image;
use iced::{Point, Rectangle, Size, Vector};
use wineo_core::runtime::State;

use super::*;
use crate::layout_probe::{bounds_at, layout, render};

pub(super) fn art_card(name: &str, state: State) -> Card {
    let mut card = crate::tests::card(name, state, 60);
    card.art = Some(crate::tests::art_file(&format!("/tmp/{name}.png"), 1));
    card
}

/// Gives `card` decoded art in one solid color.
pub(super) fn decoded(app: &mut App, card: &Card, rgb: [u8; 3]) {
    let key = (card.art.clone().expect("art"), crate::artwork::Kind::Grid);
    let pixel = [rgb[0], rgb[1], rgb[2], 255];
    app.images.insert(
        key,
        card.not_installed(),
        image::Handle::from_rgba(2, 3, pixel.repeat(6)),
    );
}

pub(super) fn card_node(app: &App, card: &Card) -> Node {
    layout(&mut app.card(card), Size::new(400.0, 900.0))
}

/// The poster's layers, bottom first.
pub(super) fn layers(node: &Node) -> Vec<Rectangle> {
    let mut layout = Layout::new(node);
    for index in [0, 0, 0] {
        layout = layout.children().nth(index).unwrap();
    }
    layout.children().map(|layer| layer.bounds()).collect()
}

/// Whether the poster-only title overlay is drawn: a layer as wide as the
/// art along its bottom, taller than the install bar.
pub(super) fn overlay_shown(node: &Node) -> bool {
    let art = bounds_at(node, &[0, 0]);
    (1..layers(node).len()).any(|index| {
        let child = bounds_at(node, &[0, 0, 0, index, 0]);
        child.width == art.width
            && child.y + child.height == art.y + art.height
            && child.height > INSTALL_BAR_HEIGHT
    })
}

/// The star's chip, centered in its 28 px cell. The star is the top layer
/// while no install bar shows.
pub(super) fn mark_chip(node: &Node) -> Rectangle {
    let cell = bounds_at(node, &[0, 0, 0, layers(node).len() - 1, 0]);
    Rectangle::new(
        cell.position() + Vector::new(2.0, 2.0),
        Size::new(24.0, 24.0),
    )
}

/// Where the star's chip sits in the art's top right corner, shown or not.
pub(super) fn star_slot(node: &Node) -> Rectangle {
    let art = bounds_at(node, &[0, 0]);
    Rectangle::new(
        Point::new(art.x + art.width - 8.0 - 24.0, art.y + 8.0),
        Size::new(24.0, 24.0),
    )
}

/// A hidden card's eye: the 24 px layer child in the art's bottom left
/// corner.
pub(super) fn eye_chip(node: &Node) -> Option<Rectangle> {
    let art = bounds_at(node, &[0, 0]);
    (1..layers(node).len())
        .map(|index| bounds_at(node, &[0, 0, 0, index, 0]))
        .find(|child| {
            child.size() == Size::new(24.0, 24.0)
                && child.x == art.x + 8.0
                && child.y + child.height == art.y + art.height - 8.0
        })
}

pub(super) fn gog(card: &mut Card) {
    card.gog = Some(crate::gog_library::GogCard {
        gog_id: Some("1".into()),
        unfinished: true,
        blocked_by: None,
        install: false,
        version: None,
    });
}

#[test]
fn the_overlay_shows_on_hover_and_selection_only() {
    let mut app = crate::tests::app();
    let card = art_card("game", State::Inactive);
    decoded(&mut app, &card, [128, 128, 128]);
    for poster_only in [false, true] {
        app.poster_only = poster_only;
        for zoom in [100, 80] {
            app.zoom = zoom;
            for (hovered, selected) in [(false, false), (true, false), (false, true), (true, true)]
            {
                app.hovered = hovered.then(|| card.name.clone());
                app.selected = selected.then(|| card.name.clone());
                let shown = overlay_shown(&card_node(&app, &card));
                assert_eq!(
                    shown,
                    poster_only && (hovered || selected),
                    "poster-only {poster_only} at {zoom}%, hovered {hovered}, selected {selected}"
                );
            }
        }
    }
}

#[test]
fn the_overlay_sits_at_the_bottom_of_the_art_in_one_or_two_lines() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    let mut card = art_card("game", State::Inactive);
    decoded(&mut app, &card, [128, 128, 128]);
    app.hovered = Some(card.name.clone());
    let line = iced::widget::text::LineHeight::default()
        .to_absolute(CARD_TITLE_SIZE.into())
        .0;
    for (title, lines) in [
        ("Portal 2", 1.0),
        ("Halo: The Master Chief Collection", 2.0),
    ] {
        card.display_title = title.into();
        let node = card_node(&app, &card);
        let art = bounds_at(&node, &[0, 0]);
        let overlay = bounds_at(&node, &[0, 0, 0, 1, 0]);
        assert_eq!(overlay.width, art.width);
        assert_eq!(overlay.y + overlay.height, art.y + art.height);
        assert_eq!(
            overlay.height,
            OVERLAY_FADE + lines * line + OVERLAY_PADDING
        );
        let label = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
        assert_eq!(label.x, art.x + OVERLAY_PADDING);
        assert!((label.height - lines * line).abs() < 0.01);
        // Clear of the install bar along the bottom.
        assert!(label.y + label.height <= art.y + art.height - INSTALL_BAR_HEIGHT);
    }
    // A long title ellipsizes on its second line, and it leaves room for
    // the Steam mark.
    card.display_title = "The Elder Scrolls V: Skyrim Special Edition Anniversary Upgrade".into();
    let font = Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    };
    let width = GridLayout::default().art_frame().width - OVERLAY_PADDING - STEAM_MARK_ROOM;
    let lines = crate::tile_title::lines(&card.display_title, width, CARD_TITLE_SIZE, font);
    assert_eq!(lines.len(), 2);
    assert!(lines[1].ends_with("..."));
    card.kind = Some(crate::library::CardKind::Proxied);
    let node = card_node(&app, &card);
    let art = bounds_at(&node, &[0, 0]);
    let label = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
    let mark = bounds_at(&node, &[0, 0, 0, 2, 0]);
    assert!(label.x + label.width <= art.x + art.width - STEAM_MARK_ROOM);
    assert_eq!(mark.x + mark.width, art.x + art.width - 8.0);
    assert!(label.x + label.width <= mark.x - 4.0);
}

#[test]
fn the_overlay_title_leaves_room_for_a_hidden_cards_eye() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    let mut card = art_card("game", State::Inactive);
    decoded(&mut app, &card, [128, 128, 128]);
    app.hovered = Some(card.name.clone());
    // Without the eye the title keeps its 8 px margin; with it, the eye's
    // margin, its width, and a 4 px gap.
    for (hidden, left) in [(false, 8.0), (true, 8.0 + 24.0 + 4.0)] {
        card.hidden = hidden;
        let node = card_node(&app, &card);
        let art = bounds_at(&node, &[0, 0]);
        let label = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
        assert_eq!(label.x, art.x + left, "hidden {hidden}");
        assert_eq!(
            label.x,
            art.x
                + if hidden {
                    EYE_MARK_ROOM
                } else {
                    OVERLAY_PADDING
                }
        );
        match eye_chip(&node) {
            Some(eye) => {
                assert!(hidden);
                assert!(eye.x + eye.width + 4.0 <= label.x + 0.01);
                // The install bar's edge stays clear under it.
                assert!(eye.y + eye.height <= art.y + art.height - INSTALL_BAR_HEIGHT);
            }
            None => assert!(!hidden),
        }
    }
    // A long title wraps in what's left, ellipsized on its second line,
    // and never reaches under the eye or the Steam mark.
    card.display_title = "The Elder Scrolls V: Skyrim Special Edition Anniversary Upgrade".into();
    card.kind = Some(crate::library::CardKind::Proxied);
    let node = card_node(&app, &card);
    let art = bounds_at(&node, &[0, 0]);
    let label = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
    let eye = eye_chip(&node).expect("eye");
    assert!(label.x >= eye.x + eye.width + 4.0 - 0.01);
    assert!(label.x + label.width <= art.x + art.width - STEAM_MARK_ROOM);
    // Not hovered or selected, the overlay is gone and the eye stays.
    app.hovered = None;
    let node = card_node(&app, &card);
    assert!(!overlay_shown(&node));
    assert!(eye_chip(&node).is_some());
}

#[test]
fn art_less_cards_keep_their_tile_title_and_loading_art_gets_the_overlay() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    let none = crate::tests::card("plain", State::Inactive, 60);
    let loading = art_card("loading", State::Inactive);
    for card in [&none, &loading] {
        app.hovered = Some(card.name.clone());
        let node = card_node(&app, card);
        let tile = bounds_at(&node, &[0, 0, 0, 0, 0]);
        if card.art.is_none() {
            // The title on the tile, and nothing over it.
            assert!(tile.width > 0.0);
            assert!(!overlay_shown(&node));
        } else {
            // A blank tile, and the title over it like the art will have.
            assert_eq!(tile.size(), Size::ZERO);
            assert!(overlay_shown(&node));
        }
    }
    // Failed art is the same as loading: no image is there.
    app.failed_images
        .insert((loading.art.clone().unwrap(), crate::artwork::Kind::Grid));
    assert!(overlay_shown(&card_node(&app, &loading)));
    app.hovered = None;
    assert!(!overlay_shown(&card_node(&app, &loading)));
    assert_eq!(layers(&card_node(&app, &loading)).len(), 1);
}

#[test]
fn the_status_chip_shows_for_any_state_but_idle() {
    let mut app = crate::tests::app();
    let mut busy = crate::tests::card("busy", State::Inactive, 60);
    busy.busy = true;
    let cards = [
        (crate::tests::card("running", State::Running, 60), true),
        (crate::tests::card("starting", State::Starting, 60), true),
        (crate::tests::card("failed", State::Failed, 60), true),
        (busy, true),
        (crate::tests::card("inactive", State::Inactive, 60), false),
        (crate::tests::card("stopped", State::Stopped, 60), false),
        (crate::tests::card("exited", State::Exited, 60), false),
    ];
    for (poster_only, zoom, chips) in [
        (true, 100, true),
        (true, 60, true),
        (false, 80, true),
        (false, 90, false),
        (false, 100, false),
    ] {
        app.poster_only = poster_only;
        app.zoom = zoom;
        for (card, signals) in &cards {
            let node = card_node(&app, card);
            assert_eq!(
                layers(&node).len() == 2,
                chips && *signals,
                "{} with poster-only {poster_only} at {zoom}%",
                card.name
            );
            if chips && *signals {
                let art = bounds_at(&node, &[0, 0]);
                let chip = bounds_at(&node, &[0, 0, 0, 1, 0]);
                assert_eq!(
                    chip,
                    Rectangle::new(
                        art.position() + Vector::new(8.0, 8.0),
                        Size::new(24.0, 24.0)
                    )
                );
            }
        }
    }
}

#[test]
fn the_chip_and_the_gog_pill_share_one_row_four_pixels_apart() {
    let mut app = crate::tests::app();
    let mut idle = crate::tests::card("idle", State::Inactive, 60);
    gog(&mut idle);
    let mut running = crate::tests::card("running", State::Running, 60);
    gog(&mut running);
    // Where 0.36.0 put the pill.
    let node = card_node(&app, &idle);
    let art = bounds_at(&node, &[0, 0]);
    let alone = bounds_at(&node, &[0, 0, 0, 1, 0]);
    assert_eq!(alone.position(), art.position() + Vector::new(8.0, 8.0));
    // The corner keeps the marks' room on every card, so beside the chip
    // "Finish update" fits on one line at 150%, wraps at 100%, and turns
    // into the badge at 80%.
    for (poster_only, zoom) in [(true, 150), (true, 100), (false, 80)] {
        app.poster_only = poster_only;
        app.zoom = zoom;
        // With no chip, the pill stays where it was.
        let node = card_node(&app, &idle);
        let art = bounds_at(&node, &[0, 0]);
        let pill = bounds_at(&node, &[0, 0, 0, 1, 0]);
        assert_eq!(pill.position(), art.position() + Vector::new(8.0, 8.0));
        let node = card_node(&app, &running);
        let art = bounds_at(&node, &[0, 0]);
        let chip = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
        let (gog, badge) = gog_at(&node, true);
        assert_eq!(chip.position(), art.position() + Vector::new(8.0, 8.0));
        assert_eq!(gog.x, chip.x + chip.width + 4.0, "{zoom}%");
        assert_eq!(badge, zoom == 80, "{zoom}%");
        if badge {
            assert_eq!(gog.y, chip.y);
            continue;
        }
        // A one-line pill centers on the chip; one that wraps grows down
        // from there, and the chip keeps its corner.
        let pill = bounds_at(&node, &[0, 0, 0, 1, 0, 1, 0]);
        assert!((pill.y - (chip.center_y() - alone.height / 2.0)).abs() < 0.01);
        if zoom == 150 {
            assert_eq!(pill.size(), alone.size());
        } else {
            assert!(pill.height > alone.height);
        }
    }
}

#[test]
fn card_heights_follow_the_text_each_zoom_shows() {
    let mut app = crate::tests::app();
    let mut card = crate::tests::card("a-long-identifier", State::Running, 60);
    card.display_title = "Halo: The Master Chief Collection".into();
    let title = crate::tile_title::two_line_height(CARD_TITLE_SIZE);
    let full = |app: &App| {
        let layout = GridLayout::new(app.zoom);
        assert_eq!(layout.text(), CardText::Full);
        card_height(&card, layout)
    };
    for (poster_only, zoom, text, height) in [
        (true, 100, CardText::PosterOnly, 240.0),
        (true, 60, CardText::PosterOnly, 144.0),
        (true, 200, CardText::PosterOnly, 480.0),
        (false, 80, CardText::Title, 192.0 + CARD_SPACING + title),
        (false, 89, CardText::Title, 213.0 + CARD_SPACING + title),
        (false, 90, CardText::Full, 0.0),
        (false, 100, CardText::Full, 0.0),
    ] {
        app.poster_only = poster_only;
        app.zoom = zoom;
        let layout = app.grid_layout();
        assert_eq!(layout.text(), text, "{zoom}%");
        let built = card_node(&app, &card).bounds().height;
        assert_eq!(card_height(&card, layout), built, "{zoom}%");
        if text == CardText::Full {
            assert_eq!(built, full(&app), "{zoom}%");
        } else {
            assert_eq!(built, height, "{zoom}%");
        }
    }
    // Below 90% the title is all that's under the poster.
    app.poster_only = false;
    app.zoom = 80;
    assert_eq!(Layout::new(&card_node(&app, &card)).children().count(), 2);
    app.zoom = 90;
    assert_eq!(Layout::new(&card_node(&app, &card)).children().count(), 4);
    app.poster_only = true;
    assert_eq!(Layout::new(&card_node(&app, &card)).children().count(), 1);
}

/// The brightest channel of the pixels in `rows` at columns `columns`.
fn brightest(
    pixels: &[u8],
    width: usize,
    rows: std::ops::Range<usize>,
    columns: std::ops::Range<usize>,
) -> u8 {
    rows.flat_map(|y| columns.clone().map(move |x| (y, x)))
        .map(|(y, x)| {
            let pixel = &pixels[(y * width + x) * 4..][..3];
            pixel.iter().copied().max().unwrap()
        })
        .max()
        .unwrap()
}

#[test]
fn the_overlay_is_dark_white_text_and_a_hidden_cards_title_is_muted() {
    use iced::theme::Mode;
    let mut app = crate::tests::app();
    app.poster_only = true;
    let mut card = art_card("game", State::Inactive);
    card.display_title = "Portal".into();
    let size = Size::new(160.0, 240.0);
    let width = 160;
    // On white art, the gradient darkens toward the bottom, and the art
    // above it stays as it is.
    decoded(&mut app, &card, [255, 255, 255]);
    app.selected = Some(card.name.clone());
    let pixels = render(&mut app.card(&card), size);
    let shade = |y: usize| brightest(&pixels, width, y..y + 1, 140..142);
    assert_eq!(shade(100), 255);
    assert!(shade(236) < shade(200), "{} {}", shade(236), shade(200));
    assert!(shade(236) <= 50, "{}", shade(236));
    // The text rows: the title's line, between the fade and the margin.
    let text = 238 - 8 - 19..238 - 8;
    let title = 10..150;
    // Hovered art dims to 85%, but the title over it stays white.
    app.selected = None;
    app.hovered = Some(card.name.clone());
    decoded(&mut app, &card, [128, 128, 128]);
    let pixels = render(&mut app.card(&card), size);
    assert!(brightest(&pixels, width, text.clone(), title.clone()) >= 250);
    // A hidden card's title is white at 60%, the same in both themes, and
    // reads at 4.5:1 or better against the gradient over its 45% art on
    // the theme's background, measured in the rendered pixels.
    card.hidden = true;
    // The title starts right of the eye, whose white icon shares its rows.
    let title = 40..150;
    let mut seen = Vec::new();
    for mode in [Mode::Dark, Mode::Light] {
        app.palette = crate::theme::Palette::from_mode(mode);
        for art in [255, 128, 0] {
            decoded(&mut app, &card, [art; 3]);
            let background = app.palette.background;
            let mut view: Element<'_, Message> = container(app.card(&card))
                .style(move |_| container::Style::default().background(background))
                .into();
            let pixels = render(&mut view, size);
            let ink = lightest(&pixels, width, text.clone(), title.clone());
            // The lightest backdrop behind the title's rows, past its end.
            let backdrop = lightest(&pixels, width, text.clone(), 140..150);
            let ratio = contrast(ink, backdrop);
            assert!(ratio >= 4.5, "{mode:?} on {art}: {ratio:.2}");
            seen.push(ink);
        }
    }
    // White at 60% over the dark gradient, never the palette's muted gray.
    assert!(
        seen.iter()
            .all(|ink| ink.iter().max().unwrap() - ink.iter().min().unwrap() <= 2)
    );
    assert!(
        seen.iter().all(|ink| (150..=180).contains(&ink[0])),
        "{seen:?}"
    );
    // A game that isn't installed, and isn't hidden, gets the same title.
    let mut absent = steam_card(
        "absent",
        State::Inactive,
        wineo_core::steam::AppState::NotInstalled,
    );
    absent.art = card.art.clone();
    absent.display_title = card.display_title.clone();
    decoded(&mut app, &absent, [128; 3]);
    app.hovered = Some(absent.name.clone());
    let pixels = render(&mut app.card(&absent), size);
    let ink = lightest(&pixels, width, text.clone(), title.clone());
    assert!((150..=180).contains(&ink[0]), "{ink:?}");
}

/// The pixel with the highest luminance in `rows` at `columns`.
fn lightest(
    pixels: &[u8],
    width: usize,
    rows: std::ops::Range<usize>,
    columns: std::ops::Range<usize>,
) -> [u8; 3] {
    rows.flat_map(|y| columns.clone().map(move |x| (y, x)))
        .map(|(y, x)| {
            let pixel = &pixels[(y * width + x) * 4..][..3];
            [pixel[0], pixel[1], pixel[2]]
        })
        .max_by(|a, b| luminance(*a).total_cmp(&luminance(*b)))
        .unwrap()
}

/// WCAG relative luminance.
fn luminance(rgb: [u8; 3]) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn every_poster_only_layer_fades_with_its_card() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    let mut card = art_card("game", State::Running);
    card.kind = Some(crate::library::CardKind::Proxied);
    gog(&mut card);
    card.favorite = true;
    decoded(&mut app, &card, [200, 200, 200]);
    app.selected = Some(card.name.clone());
    let size = Size::new(200.0, 300.0);
    let layout = app.grid_layout();
    let shown = render(
        &mut app.card_drawn(&card, 1.0, layout, Details::Inline),
        size,
    );
    assert!(shown.chunks(4).any(|pixel| pixel[3] > 0));
    let gone = render(
        &mut app.card_drawn(&card, 0.0, layout, Details::Inline),
        size,
    );
    assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));
}

fn steam_card(name: &str, state: State, install: wineo_core::steam::AppState) -> Card {
    let mut card = crate::tests::card(name, state, 60);
    card.kind = Some(crate::library::CardKind::Proxied);
    card.steam = Some(crate::library::SteamDetails {
        install: install.into(),
        proton: None,
        size: None,
    });
    card
}

#[test]
fn every_state_the_row_shows_as_activity_or_a_warning_gets_the_chip_in_its_dot_color() {
    use wineo_core::steam::AppState;
    let mut app = crate::tests::app();
    let mut busy = crate::tests::card("busy", State::Inactive, 60);
    busy.busy = true;
    let mut error = crate::tests::card("error", State::Inactive, 60);
    error.error = Some("broken".into());
    // Each card, what its state row says, and whether that's at rest.
    let cards = [
        (
            crate::tests::card("a", State::Running, 60),
            "running",
            false,
        ),
        (
            crate::tests::card("b", State::Starting, 60),
            "starting",
            false,
        ),
        (
            crate::tests::card("c", State::SteamRunning, 60),
            "running (started from Steam)",
            false,
        ),
        (crate::tests::card("d", State::Failed, 60), "failed", false),
        (busy, "busy", false),
        (error, "error", false),
        (crate::tests::card("e", State::Exited, 60), "exited", true),
        (crate::tests::card("f", State::Stopped, 60), "stopped", true),
        (
            crate::tests::card("g", State::Inactive, 60),
            "inactive",
            true,
        ),
        (
            steam_card("h", State::Inactive, AppState::Installed),
            "inactive",
            true,
        ),
        (
            steam_card("i", State::Inactive, AppState::NotInstalled),
            "not installed",
            true,
        ),
        (
            steam_card("j", State::Inactive, AppState::Updating),
            "updating",
            false,
        ),
        (
            steam_card("k", State::Inactive, AppState::Installing(Some(40))),
            "installing",
            false,
        ),
        (
            steam_card("l", State::Inactive, AppState::Installing(None)),
            "installing",
            false,
        ),
        (
            steam_card("m", State::Inactive, AppState::InstallQueued),
            "install queued",
            false,
        ),
        (
            steam_card("n", State::Inactive, AppState::Uninstalling),
            "uninstalling",
            false,
        ),
        (
            steam_card("o", State::Inactive, AppState::Unavailable("/lib".into())),
            "unavailable",
            false,
        ),
        (
            steam_card("p", State::Inactive, AppState::Unknown("no Steam".into())),
            "unknown",
            false,
        ),
        (
            steam_card("q", State::Running, AppState::Updating),
            "running",
            false,
        ),
    ];
    for (poster_only, zoom) in [(true, 100), (false, 80)] {
        app.poster_only = poster_only;
        app.zoom = zoom;
        let size = Size::new(200.0, 400.0);
        for (card, word, idle) in &cards {
            assert_eq!(card.state_word(), *word, "{}", card.name);
            assert_eq!(card.idle(), *idle, "{word}");
            let node = card_node(&app, card);
            let art = bounds_at(&node, &[0, 0]);
            let chip = (1..layers(&node).len())
                .map(|index| bounds_at(&node, &[0, 0, 0, index, 0]))
                .find(|first| {
                    *first
                        == Rectangle::new(
                            art.position() + Vector::new(8.0, 8.0),
                            Size::new(24.0, 24.0),
                        )
                });
            assert_eq!(chip.is_some(), !idle, "{word} at {zoom}%");
            if *idle {
                continue;
            }
            let center = chip.unwrap().center();
            let pixels = render(&mut app.card(card), size);
            let at = (center.y as usize * 200 + center.x as usize) * 4;
            assert_eq!(
                pixels[at..at + 3],
                card.dot(app.palette).into_rgba8()[..3],
                "{word}"
            );
        }
    }
}

/// The GOG pill's or badge's bounds in the corner, and whether it's the
/// badge.
pub(super) fn gog_at(node: &Node, chip: bool) -> (Rectangle, bool) {
    let bounds = if chip {
        bounds_at(node, &[0, 0, 0, 1, 0, 1])
    } else {
        bounds_at(node, &[0, 0, 0, 1, 0])
    };
    (bounds, bounds.size() == Size::new(24.0, 24.0))
}

#[test]
fn the_corner_keeps_clear_of_the_marks() {
    let mut app = crate::tests::app();
    for (favorite, hidden) in [(true, false), (false, true), (true, true)] {
        for (poster_only, zoom) in [(true, 60), (true, 200), (false, 60), (false, 200)] {
            app.poster_only = poster_only;
            app.zoom = zoom;
            for state in [State::Running, State::Inactive] {
                let mut card = crate::tests::card("game", state, 60);
                gog(&mut card);
                card.favorite = favorite;
                card.hidden = hidden;
                let what = format!(
                    "favorite {favorite}, hidden {hidden}, poster-only {poster_only} at {zoom}%, {state:?}"
                );
                let node = card_node(&app, &card);
                // The star's chip, where it shows or not.
                let marks = star_slot(&node);
                let chip = state == State::Running && app.grid_layout().text().chip();
                let (gog, badge) = gog_at(&node, chip);
                assert!(gog.width > 0.0, "{what}");
                // A pill keeps a 4 px gap; a badge at 60% has to give it
                // up, but never overlaps.
                let gap = if badge { 0.0 } else { 4.0 };
                assert!(
                    gog.x + gog.width <= marks.x - gap + 0.01,
                    "{what}: {gog:?} against {marks:?}"
                );
                if zoom == 200 {
                    // Room to spare: the pill keeps its one-line size.
                    let mut plain = card.clone();
                    plain.favorite = false;
                    plain.hidden = false;
                    let (alone, _) = gog_at(&card_node(&app, &plain), chip);
                    assert!(!badge, "{what}");
                    assert_eq!(gog.size(), alone.size(), "{what}");
                }
                if zoom == 60 && chip && favorite && hidden {
                    assert!(badge, "{what}");
                }
            }
        }
    }
}

#[test]
fn a_pill_too_small_for_its_words_becomes_a_round_badge() {
    let mut app = crate::tests::app();
    app.poster_only = true;
    app.zoom = 60;
    let mut card = crate::tests::card("game", State::Running, 60);
    gog(&mut card);
    card.favorite = true;
    card.hidden = true;
    // With the chip and both marks: 4 px right of the chip, in its row.
    let node = card_node(&app, &card);
    let art = bounds_at(&node, &[0, 0]);
    let chip = bounds_at(&node, &[0, 0, 0, 1, 0, 0]);
    let (badge, is_badge) = gog_at(&node, true);
    assert!(is_badge);
    assert_eq!(badge.position(), chip.position() + Vector::new(28.0, 0.0));
    assert_eq!(chip.position(), art.position() + Vector::new(8.0, 8.0));
    assert!(badge.x + badge.width <= mark_chip(&node).x);
    // The pill's color, with a down arrow in its ink at the center.
    let size = Size::new(96.0, 144.0);
    let pixels = render(&mut app.card(&card), size);
    let pixel = |x: f32, y: f32| {
        let at = (y as usize * 96 + x as usize) * 4;
        [pixels[at], pixels[at + 1], pixels[at + 2]]
    };
    let warning = app.palette.warning.into_rgba8();
    assert_eq!(pixel(badge.x + 4.0, badge.center_y()), warning[..3]);
    // The arrow's stroke is under 2 px, so its darkest pixel near the
    // center comes close to the ink without matching it.
    let ink = ui::accent_ink(app.palette.warning).into_rgba8();
    let closest = (-3..=3)
        .flat_map(|dx| (-3..=3).map(move |dy| (dx, dy)))
        .map(|(dx, dy)| pixel(badge.center_x() + dx as f32, badge.center_y() + dy as f32))
        .map(|rgb| {
            rgb.iter()
                .zip(&ink)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap()
        })
        .min()
        .unwrap();
    assert!(closest <= 24, "{closest}");
    // In the corner when there's no chip.
    card.state = State::Inactive;
    let node = card_node(&app, &card);
    let (badge, is_badge) = gog_at(&node, false);
    assert!(is_badge);
    assert_eq!(badge.position(), art.position() + Vector::new(8.0, 8.0));
    // It fades with the card.
    let layout = app.grid_layout();
    let gone = render(
        &mut app.card_drawn(&card, 0.0, layout, Details::Inline),
        size,
    );
    assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));
}

#[test]
fn the_pill_keeps_its_words_wherever_they_fit() {
    let mut app = crate::tests::app();
    let mut card = crate::tests::card("game", State::Running, 60);
    gog(&mut card);
    card.favorite = true;
    card.hidden = true;
    // 0.36.0's pill at 100%.
    let (today, _) = gog_at(&card_node(&app, &card), false);
    for zoom in [100, 200] {
        for poster_only in [false, true] {
            app.zoom = zoom;
            app.poster_only = poster_only;
            let chip = app.grid_layout().text().chip();
            let node = card_node(&app, &card);
            let (pill, badge) = gog_at(&node, chip);
            assert!(!badge, "{zoom}%, poster-only {poster_only}");
            let pill = if chip {
                bounds_at(&node, &[0, 0, 0, 1, 0, 1, 0])
            } else {
                pill
            };
            // At 100% beside the chip and the marks, "Finish update" is
            // 2 px too wide for one line, so it wraps between its words.
            if chip && zoom == 100 {
                assert!(pill.height > today.height);
            } else {
                assert_eq!(
                    pill.size(),
                    today.size(),
                    "{zoom}%, poster-only {poster_only}"
                );
            }
        }
    }
    // The choice follows the measured words: the badge only when the
    // longest won't fit.
    assert!(pill_fits("Finish update", 16.0 + 37.0));
    assert!(!pill_fits("Finish update", 16.0 + 36.0));
}
