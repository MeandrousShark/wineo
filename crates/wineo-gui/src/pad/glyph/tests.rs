//! The glyphs' look: which SVG or pill each button gets, what the SVGs
//! really draw, and the contrast their colors keep.

use super::*;
use crate::theme::contrast;

const STYLES: [Style; 4] = [
    Style::Xbox,
    Style::PlayStation,
    Style::Nintendo,
    Style::GameCube,
];

/// The faces each style draws for A, B, X, and Y, with their edges.
fn faces(style: Style) -> [(&'static [u8], &'static [u8]); 4] {
    match style {
        Style::GameCube => [
            (GC_A, GC_A_EDGE),
            (GC_B, GC_B_EDGE),
            (GC_X, GC_X_EDGE),
            (GC_Y, GC_Y_EDGE),
        ],
        Style::Xbox => [XBOX_A, XBOX_B, XBOX_X, XBOX_Y].map(|svg| (svg, DISC_EDGE)),
        Style::PlayStation => {
            [PS_CROSS, PS_CIRCLE, PS_SQUARE, PS_TRIANGLE].map(|svg| (svg, DISC_EDGE))
        }
        Style::Nintendo => {
            [NINTENDO_A, NINTENDO_B, NINTENDO_X, NINTENDO_Y].map(|svg| (svg, DISC_EDGE))
        }
    }
}

#[test]
fn each_face_button_gets_its_own_styles_svg_and_edge() {
    for style in STYLES {
        for (button, (svg, edge)) in [PadButton::A, PadButton::B, PadButton::X, PadButton::Y]
            .into_iter()
            .zip(faces(style))
        {
            assert_eq!(
                glyph(style, button),
                Glyph::Face { svg, edge },
                "{style:?} {button:?}"
            );
        }
    }
    // Sixteen different faces, one per button per style.
    let mut all: Vec<&[u8]> = STYLES
        .into_iter()
        .flat_map(|style| faces(style).map(|(svg, _)| svg))
        .collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 16);
}

#[test]
fn each_label_gets_its_pill_colors() {
    let grey = (GAMECUBE_GREY, DARK_TEXT);
    let purple = (GAMECUBE_PURPLE, WHITE);
    let yellow = (GAMECUBE_YELLOW, DARK_TEXT);
    let dark = (DARK, LIGHT_TEXT);
    let expected = |style, button| match (style, button) {
        (Style::GameCube, PadButton::Lb | PadButton::Rb | PadButton::Start) => grey,
        (Style::GameCube, PadButton::View) => purple,
        (Style::GameCube, PadButton::Lt | PadButton::Rt) => yellow,
        _ => dark,
    };
    for style in STYLES {
        for button in PadButton::ALL {
            let colors = match glyph(style, button) {
                Glyph::Face { .. } => {
                    assert!(
                        matches!(
                            button,
                            PadButton::A | PadButton::B | PadButton::X | PadButton::Y
                        ),
                        "{style:?} {button:?}"
                    );
                    continue;
                }
                Glyph::Label { fill, ink, .. } => (fill, ink),
            };
            assert_eq!(colors, expected(style, button), "{style:?} {button:?}");
        }
    }
    assert_eq!(GAMECUBE_GREY, Rgb(0xc4c6cc));
    assert_eq!(GAMECUBE_PURPLE, Rgb(0x6a55c8));
    assert_eq!(GAMECUBE_YELLOW, Rgb(0xf2c318));
}

/// A point in a face's 24 unit box.
type Point = (f32, f32);

/// Where to look in a face, in its 24 unit box: a point on the button
/// clear of the mark, with the fill's color, and one on the mark, with the
/// mark's.
struct Probe {
    svg: &'static [u8],
    fill: (Point, u32),
    mark: (Point, u32),
}

/// The disc faces' fill point sits above the mark.
const DISC_FILL: (Point, u32) = ((12.0, 4.0), 0x1f2024);
/// A point on each mark the discs carry.
const ON_A: Point = (12.0, 13.5);
const ON_B: Point = (9.0, 12.0);
const ON_X: Point = (12.0, 12.0);
const ON_Y: Point = (12.0, 15.0);
const ON_CROSS: Point = (12.0, 12.0);
const ON_CIRCLE: Point = (12.0, 7.4);
const ON_SQUARE: Point = (8.0, 12.0);
const ON_TRIANGLE: Point = (12.0, 15.5);

fn disc(svg: &'static [u8], mark: Point, color: u32) -> Probe {
    Probe {
        svg,
        fill: DISC_FILL,
        mark: (mark, color),
    }
}

fn probes() -> Vec<Probe> {
    vec![
        Probe {
            svg: GC_A,
            fill: ((12.0, 4.0), 0x16a488),
            mark: (ON_A, 0xffffff),
        },
        Probe {
            svg: GC_B,
            fill: ((12.0, 5.2), 0xdc2f3a),
            // B's stem, the mark drawn at 0.8 about the center.
            mark: ((9.6, 12.0), 0xffffff),
        },
        Probe {
            svg: GC_X,
            // Up the bean from the mark, and the mark's crossing.
            fill: ((11.7, 7.07), 0xc4c6cc),
            mark: ((13.26, 12.0), 0x2a2b30),
        },
        Probe {
            svg: GC_Y,
            fill: ((7.07, 12.3), 0xc4c6cc),
            mark: ((12.0, 12.5), 0x2a2b30),
        },
        disc(XBOX_A, ON_A, 0x6cc04a),
        disc(XBOX_B, ON_B, 0xe5343b),
        disc(XBOX_X, ON_X, 0x2f8fe6),
        disc(XBOX_Y, ON_Y, 0xf4c20d),
        disc(PS_CROSS, ON_CROSS, 0x7c9fe0),
        disc(PS_CIRCLE, ON_CIRCLE, 0xe8505b),
        disc(PS_SQUARE, ON_SQUARE, 0xd48fc7),
        disc(PS_TRIANGLE, ON_TRIANGLE, 0x40c8b0),
        disc(NINTENDO_A, ON_A, 0xf2f3f5),
        disc(NINTENDO_B, ON_B, 0xf2f3f5),
        disc(NINTENDO_X, ON_X, 0xf2f3f5),
        disc(NINTENDO_Y, ON_Y, 0xf2f3f5),
    ]
}

/// Draws are at twice the 24 unit box, so a point lands well inside a
/// stroke.
const SCALE: f32 = 2.0;

/// `svg` rasterized with the crate's own resvg at 48 px.
fn raster(svg: &[u8]) -> resvg::tiny_skia::Pixmap {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).unwrap();
    let size = (24.0 * SCALE) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// The straight RGBA at `point` in the box.
fn pixel(pixmap: &resvg::tiny_skia::Pixmap, (x, y): Point) -> [u8; 4] {
    let color = pixmap
        .pixel((x * SCALE) as u32, (y * SCALE) as u32)
        .unwrap()
        .demultiply();
    [color.red(), color.green(), color.blue(), color.alpha()]
}

fn assert_color(found: [u8; 4], hex: u32, what: &str) {
    let [_, r, g, b] = hex.to_be_bytes();
    let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
    assert!(
        found[3] == 255 && close(found[0], r) && close(found[1], g) && close(found[2], b),
        "{what}: found {found:?}, wanted #{hex:06x}"
    );
}

#[test]
fn each_face_draws_its_fill_and_its_mark_in_their_colors() {
    let probes = probes();
    assert_eq!(probes.len(), 16, "every face is probed");
    for (index, probe) in probes.iter().enumerate() {
        let pixmap = raster(probe.svg);
        let (at, hex) = probe.fill;
        assert_color(pixel(&pixmap, at), hex, &format!("face {index}'s fill"));
        let (at, hex) = probe.mark;
        assert_color(pixel(&pixmap, at), hex, &format!("face {index}'s mark"));
    }
}

#[test]
fn each_edge_is_a_ring_around_its_face() {
    // A point in the ring, and the middle it leaves open, so a dimmed face
    // never shows its edge through the fill.
    let edges: [(&[u8], Point, Point); 5] = [
        (DISC_EDGE, (12.0, 12.0 - 10.5), (12.0, 12.0)),
        (GC_A_EDGE, (12.0, 12.0 - 11.1), (12.0, 12.0)),
        (GC_B_EDGE, (12.0, 12.0 - 8.7), (12.0, 12.0)),
        (GC_X_EDGE, (17.96, 12.0), (13.26, 12.0)),
        (GC_Y_EDGE, (12.0, 6.04), (12.0, 10.74)),
    ];
    for (index, (svg, ring, middle)) in edges.into_iter().enumerate() {
        let pixmap = raster(svg);
        assert_eq!(pixel(&pixmap, ring)[3], 255, "edge {index}'s ring");
        assert_eq!(pixel(&pixmap, middle)[3], 0, "edge {index}'s middle");
    }
}

#[test]
fn faces_draw_untinted_and_edges_in_the_muted_color() {
    assert_eq!(face_style().color, None);
    for preset in crate::theme::PRESETS {
        let palette = preset.palette;
        assert_eq!(
            edge_style(palette).color,
            Some(palette.muted),
            "{}",
            preset.id
        );
    }
}

#[test]
fn every_mark_and_label_keeps_its_contrast() {
    let color = |hex: u32| Rgb(hex).color();
    for probe in probes() {
        let ratio = contrast(color(probe.fill.1), color(probe.mark.1));
        assert!(
            ratio >= 3.0,
            "#{:06x} on #{:06x}: {ratio}",
            probe.mark.1,
            probe.fill.1
        );
    }
    for style in STYLES {
        for button in PadButton::ALL {
            if let Glyph::Label { text, fill, ink } = glyph(style, button) {
                let ratio = contrast(fill.color(), ink.color());
                assert!(ratio >= 4.5, "{style:?} {text}: {ratio}");
            }
        }
    }
}
