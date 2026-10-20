//! Cards in the poster-only view and with the compact text below 90%: the
//! title overlay, the status chip beside the GOG pill, and their heights.

use iced::advanced::Layout;
use iced::advanced::layout::Node;
use iced::widget::image;
use iced::{Rectangle, Size, Vector};
use wineo_core::runtime::State;

use super::*;
use crate::layout_probe::{bounds_at, layout, render};

fn art_card(name: &str, state: State) -> Card {
    let mut card = crate::tests::card(name, state, 60);
    card.art = Some(crate::tests::art_file(&format!("/tmp/{name}.png"), 1));
    card
}

/// Gives `card` decoded art in one solid color.
fn decoded(app: &mut App, card: &Card, rgb: [u8; 3]) {
    let key = (card.art.clone().expect("art"), crate::artwork::Kind::Grid);
    let pixel = [rgb[0], rgb[1], rgb[2], 255];
    app.images.insert(
        key,
        card.not_installed(),
        image::Handle::from_rgba(2, 3, pixel.repeat(6)),
    );
}

fn card_node(app: &App, card: &Card) -> Node {
    layout(&mut app.card(card), Size::new(400.0, 900.0))
}

/// The poster's layers, bottom first.
fn layers(node: &Node) -> Vec<Rectangle> {
    let mut layout = Layout::new(node);
    for index in [0, 0, 0] {
        layout = layout.children().nth(index).unwrap();
    }
    layout.children().map(|layer| layer.bounds()).collect()
}

fn gog(card: &mut Card) {
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
                let shown = layers(&card_node(&app, &card)).len() == 2;
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
            assert_eq!(layers(&node).len(), 1);
        } else {
            // A blank tile, and the title over it like the art will have.
            assert_eq!(tile.size(), Size::ZERO);
            assert_eq!(layers(&node).len(), 2);
        }
    }
    // Failed art is the same as loading: no image is there.
    app.failed_images
        .insert((loading.art.clone().unwrap(), crate::artwork::Kind::Grid));
    assert_eq!(layers(&card_node(&app, &loading)).len(), 2);
    app.hovered = None;
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
    for (poster_only, zoom) in [(true, 100), (false, 80)] {
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
        let pill = bounds_at(&node, &[0, 0, 0, 1, 0, 1, 0]);
        assert_eq!(chip.position(), art.position() + Vector::new(8.0, 8.0));
        assert_eq!(pill.x, chip.x + chip.width + 4.0);
        // A one-line pill centers on the chip; one that wraps on a small
        // poster grows down from there, and the chip keeps its corner.
        assert!((pill.y - (chip.center_y() - alone.height / 2.0)).abs() < 0.01);
        if zoom == 100 {
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
    card.hidden = true;
    for mode in [Mode::Dark, Mode::Light] {
        app.palette = crate::theme::Palette::from_mode(mode);
        let muted = app.palette.muted.into_rgba8();
        let pixels = render(&mut app.card(&card), size);
        let brightest = brightest(&pixels, width, text.clone(), title.clone());
        let expected = muted[..3].iter().copied().max().unwrap();
        assert!(
            brightest.abs_diff(expected) <= 4,
            "{mode:?}: {brightest} against {expected}"
        );
    }
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
    let shown = render(&mut app.card_drawn(&card, 1.0, layout), size);
    assert!(shown.chunks(4).any(|pixel| pixel[3] > 0));
    let gone = render(&mut app.card_drawn(&card, 0.0, layout), size);
    assert!(gone.chunks(4).all(|pixel| pixel[3] == 0));
}
