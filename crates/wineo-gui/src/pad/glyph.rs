//! Button glyphs and the hint bar. Face buttons are drawn SVGs, since Noto
//! Sans lacks the PlayStation shapes; the rest are short labels in a pill.
//! Every glyph has its button's own colors, the same in every theme, since
//! it stands for a physical button, and an edge in the theme's muted color
//! so a dark button shows on a dark surface and a light one on a light
//! surface.

use iced::widget::{Row, container, row, stack, svg, text};
use iced::{Alignment, Color, Element, border};
use wineo_core::library::{Bindings, PadAction, PadButton};

use super::Style;
use crate::theme::Palette;

/// The bar's height, which the grid keeps clear under its last row.
pub(crate) const HINT_HEIGHT: f32 = 32.0;
const GLYPH: f32 = 20.0;

// Each face is its button's fill and mark in their own colors, drawn
// untinted. In the 24 unit box drawn at 20 px, 1.2 units is one pixel.
const GC_A: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.8" fill="#16a488"/><path d="M8.5 16.5L12 7.5l3.5 9M9.7 13.5h4.6" fill="none" stroke="#ffffff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const GC_B: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.4" fill="#dc2f3a"/><path transform="translate(12 12) scale(0.8) translate(-12 -12)" d="M9 7.5v9h3.8a2.3 2.3 0 0 0 0-4.6H9M9 7.5h3.3a2.2 2.2 0 0 1 0 4.4" fill="none" stroke="#ffffff" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const GC_X: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M13.85 2.81A13 13 0 0 1 13.85 21.19A4.4 4.4 0 0 1 7.63 14.97A4.2 4.2 0 0 0 7.63 9.03A4.4 4.4 0 0 1 13.85 2.81z" fill="#c4c6cc"/><path transform="translate(13.26 12) scale(0.7) translate(-12 -12)" d="M8.8 7.8l6.4 8.4M15.2 7.8l-6.4 8.4" fill="none" stroke="#2a2b30" stroke-width="2.57" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const GC_Y: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path transform="rotate(-90 12 12)" d="M13.85 2.81A13 13 0 0 1 13.85 21.19A4.4 4.4 0 0 1 7.63 14.97A4.2 4.2 0 0 0 7.63 9.03A4.4 4.4 0 0 1 13.85 2.81z" fill="#c4c6cc"/><path transform="translate(12 10.59) scale(0.7) translate(-12 -12)" d="M8.8 7.8L12 12l3.2-4.2M12 12v4.5" fill="none" stroke="#2a2b30" stroke-width="2.57" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const XBOX_A: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.5 16.5L12 7.5l3.5 9M9.7 13.5h4.6" fill="none" stroke="#6cc04a" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const XBOX_B: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M9 7.5v9h3.8a2.3 2.3 0 0 0 0-4.6H9M9 7.5h3.3a2.2 2.2 0 0 1 0 4.4" fill="none" stroke="#e5343b" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const XBOX_X: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.8 7.8l6.4 8.4M15.2 7.8l-6.4 8.4" fill="none" stroke="#2f8fe6" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const XBOX_Y: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.8 7.8L12 12l3.2-4.2M12 12v4.5" fill="none" stroke="#f4c20d" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const PS_CROSS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.3 8.3l7.4 7.4M15.7 8.3l-7.4 7.4" fill="none" stroke="#7c9fe0" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const PS_CIRCLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M12 7.4a4.6 4.6 0 1 0 0 9.2a4.6 4.6 0 1 0 0-9.2z" fill="none" stroke="#e8505b" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const PS_SQUARE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8 8h8v8H8z" fill="none" stroke="#d48fc7" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const PS_TRIANGLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M12 7.2l4.8 8.3H7.2z" fill="none" stroke="#40c8b0" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const NINTENDO_A: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.5 16.5L12 7.5l3.5 9M9.7 13.5h4.6" fill="none" stroke="#f2f3f5" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const NINTENDO_B: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M9 7.5v9h3.8a2.3 2.3 0 0 0 0-4.6H9M9 7.5h3.3a2.2 2.2 0 0 1 0 4.4" fill="none" stroke="#f2f3f5" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const NINTENDO_X: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.8 7.8l6.4 8.4M15.2 7.8l-6.4 8.4" fill="none" stroke="#f2f3f5" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const NINTENDO_Y: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.2" fill="#1f2024"/><path d="M8.8 7.8L12 12l3.2-4.2M12 12v4.5" fill="none" stroke="#f2f3f5" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

// Each face's edge: a ring one pixel wide around it, reaching under its
// fill so no seam shows, tinted with the theme's muted color.
const DISC_EDGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill-rule="evenodd" d="M0.6 12a11.4 11.4 0 1 0 22.8 0a11.4 11.4 0 1 0 -22.8 0zM2.4 12a9.6 9.6 0 1 0 19.2 0a9.6 9.6 0 1 0 -19.2 0z"/></svg>"##;
const GC_A_EDGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill-rule="evenodd" d="M0 12a12 12 0 1 0 24 0a12 12 0 1 0 -24 0zM1.8 12a10.2 10.2 0 1 0 20.4 0a10.2 10.2 0 1 0 -20.4 0z"/></svg>"##;
const GC_B_EDGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill-rule="evenodd" d="M2.4 12a9.6 9.6 0 1 0 19.2 0a9.6 9.6 0 1 0 -19.2 0zM4.2 12a7.8 7.8 0 1 0 15.6 0a7.8 7.8 0 1 0 -15.6 0z"/></svg>"##;
const GC_X_EDGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill-rule="evenodd" d="M14.7 1.96A14.2 14.2 0 0 1 14.7 22.04A5.6 5.6 0 0 1 6.78 14.12A3 3 0 0 0 6.78 9.88A5.6 5.6 0 0 1 14.7 1.96zM13.43 3.23A12.4 12.4 0 0 1 13.43 20.77A3.8 3.8 0 0 1 8.05 15.39A4.8 4.8 0 0 0 8.05 8.61A3.8 3.8 0 0 1 13.43 3.23z"/></svg>"##;
const GC_Y_EDGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path transform="rotate(-90 12 12)" fill-rule="evenodd" d="M14.7 1.96A14.2 14.2 0 0 1 14.7 22.04A5.6 5.6 0 0 1 6.78 14.12A3 3 0 0 0 6.78 9.88A5.6 5.6 0 0 1 14.7 1.96zM13.43 3.23A12.4 12.4 0 0 1 13.43 20.77A3.8 3.8 0 0 1 8.05 15.39A4.8 4.8 0 0 0 8.05 8.61A3.8 3.8 0 0 1 13.43 3.23z"/></svg>"##;

/// A color as `0xRRGGBB`, which keeps `Glyph` comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb(pub(crate) u32);

impl Rgb {
    pub(crate) fn color(self) -> Color {
        let [_, r, g, b] = self.0.to_be_bytes();
        Color::from_rgb8(r, g, b)
    }
}

/// The dark button most pads have, and the text that goes on it.
const DARK: Rgb = Rgb(0x1f2024);
const LIGHT_TEXT: Rgb = Rgb(0xf2f3f5);
const DARK_TEXT: Rgb = Rgb(0x2a2b30);
const WHITE: Rgb = Rgb(0xffffff);
/// A GameCube pad's L, R, and Start.
const GAMECUBE_GREY: Rgb = Rgb(0xc4c6cc);
const GAMECUBE_PURPLE: Rgb = Rgb(0x6a55c8);
/// The C-stick's yellow.
const GAMECUBE_YELLOW: Rgb = Rgb(0xf2c318);

/// How a button looks: a face's SVG and its edge, or a label's text with
/// its pill's fill and the text's color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    Face {
        svg: &'static [u8],
        edge: &'static [u8],
    },
    Label {
        text: &'static str,
        fill: Rgb,
        ink: Rgb,
    },
}

/// How a pad of `style` shows `button`. `button` is the printed label in
/// Xbox terms, so a Nintendo A and an Xbox A both read A, and a
/// PlayStation pad's cross is its A.
pub(crate) fn glyph(style: Style, button: PadButton) -> Glyph {
    let face = |svg, edge| Glyph::Face { svg, edge };
    let disc = |svg| face(svg, DISC_EDGE);
    match (style, button) {
        (Style::GameCube, PadButton::A) => face(GC_A, GC_A_EDGE),
        (Style::GameCube, PadButton::B) => face(GC_B, GC_B_EDGE),
        (Style::GameCube, PadButton::X) => face(GC_X, GC_X_EDGE),
        (Style::GameCube, PadButton::Y) => face(GC_Y, GC_Y_EDGE),
        (Style::Xbox, PadButton::A) => disc(XBOX_A),
        (Style::Xbox, PadButton::B) => disc(XBOX_B),
        (Style::Xbox, PadButton::X) => disc(XBOX_X),
        (Style::Xbox, PadButton::Y) => disc(XBOX_Y),
        (Style::PlayStation, PadButton::A) => disc(PS_CROSS),
        (Style::PlayStation, PadButton::B) => disc(PS_CIRCLE),
        (Style::PlayStation, PadButton::X) => disc(PS_SQUARE),
        (Style::PlayStation, PadButton::Y) => disc(PS_TRIANGLE),
        (Style::Nintendo, PadButton::A) => disc(NINTENDO_A),
        (Style::Nintendo, PadButton::B) => disc(NINTENDO_B),
        (Style::Nintendo, PadButton::X) => disc(NINTENDO_X),
        (Style::Nintendo, PadButton::Y) => disc(NINTENDO_Y),
        _ => {
            let (fill, ink) = pill_colors(style, button);
            Glyph::Label {
                text: label(style, button),
                fill,
                ink,
            }
        }
    }
}

/// The word on a button that isn't a face.
fn label(style: Style, button: PadButton) -> &'static str {
    let playstation = style == Style::PlayStation;
    let nintendo = style == Style::Nintendo;
    let gamecube = style == Style::GameCube;
    match button {
        PadButton::Lb if playstation => "L1",
        PadButton::Rb if playstation => "R1",
        PadButton::Lt if playstation => "L2",
        PadButton::Rt if playstation => "R2",
        PadButton::View if playstation => "Create",
        PadButton::Start if playstation => "Options",
        PadButton::Ls if playstation => "L3",
        PadButton::Rs if playstation => "R3",
        PadButton::Lb if gamecube => "L",
        PadButton::Rb if gamecube => "R",
        PadButton::View if gamecube => "Z",
        PadButton::Start if gamecube => "Start",
        PadButton::Lt if gamecube => "C left",
        PadButton::Rt if gamecube => "C right",
        PadButton::Lb if nintendo => "L",
        PadButton::Rb if nintendo => "R",
        PadButton::Lt if nintendo => "ZL",
        PadButton::Rt if nintendo => "ZR",
        PadButton::View if nintendo => "-",
        PadButton::Start if nintendo => "+",
        PadButton::Lb => "LB",
        PadButton::Rb => "RB",
        PadButton::Lt => "LT",
        PadButton::Rt => "RT",
        PadButton::View => "View",
        PadButton::Start => "Menu",
        PadButton::Ls => "LS",
        PadButton::Rs => "RS",
        PadButton::A => "A",
        PadButton::B => "B",
        PadButton::X => "X",
        PadButton::Y => "Y",
    }
}

/// A label's pill and text colors: dark with light text, except where a
/// GameCube pad colors its own.
fn pill_colors(style: Style, button: PadButton) -> (Rgb, Rgb) {
    match (style, button) {
        (Style::GameCube, PadButton::Lb | PadButton::Rb | PadButton::Start) => {
            (GAMECUBE_GREY, DARK_TEXT)
        }
        (Style::GameCube, PadButton::View) => (GAMECUBE_PURPLE, WHITE),
        (Style::GameCube, PadButton::Lt | PadButton::Rt) => (GAMECUBE_YELLOW, DARK_TEXT),
        _ => (DARK, LIGHT_TEXT),
    }
}

/// The actions the bar names, in its order.
const HINTED: [PadAction; 5] = [
    PadAction::Confirm,
    PadAction::Back,
    PadAction::Favorite,
    PadAction::Mark,
    PadAction::Random,
];

/// What the bar says for each bound action: the button and its word. On a
/// section's header or card there's no game to favorite or mark.
pub(crate) fn hints(
    bindings: &Bindings,
    panel_open: bool,
    on_header: bool,
) -> Vec<(PadButton, &'static str)> {
    HINTED
        .into_iter()
        .filter(|action| !(on_header && matches!(action, PadAction::Favorite | PadAction::Mark)))
        .filter_map(|action| {
            let word = match action {
                PadAction::Confirm if panel_open => "Play",
                PadAction::Confirm => "Open",
                PadAction::Back => "Back",
                PadAction::Favorite => "Favorite",
                PadAction::Mark => "Mark",
                _ => "Random",
            };
            Some((bindings.button(action)?, word))
        })
        .collect()
}

/// How a face is drawn: untinted, in its own colors.
pub(crate) fn face_style() -> svg::Style {
    svg::Style { color: None }
}

/// How a face's edge is drawn: tinted with the theme's muted color, which
/// stands out from the surface in every theme.
pub(crate) fn edge_style(palette: Palette) -> svg::Style {
    svg::Style {
        color: Some(palette.muted),
    }
}

/// A button's glyph at `opacity`, which a locked remap row lowers.
pub(crate) fn glyph_view<'a, M: 'a>(
    style: Style,
    button: PadButton,
    palette: Palette,
    opacity: f32,
) -> Element<'a, M> {
    match glyph(style, button) {
        Glyph::Face { svg: face, edge } => {
            let drawn = |bytes| {
                svg(svg::Handle::from_memory(bytes))
                    .width(GLYPH)
                    .height(GLYPH)
                    .opacity(opacity)
            };
            stack![
                drawn(edge).style(move |_, _| edge_style(palette)),
                drawn(face).style(|_, _| face_style()),
            ]
            .into()
        }
        Glyph::Label {
            text: label,
            fill,
            ink,
        } => container(
            text(label)
                .size(11)
                .font(crate::fonts::semibold())
                .color(ink.color().scale_alpha(opacity)),
        )
        .padding([2, 6])
        .style(move |_| {
            container::Style::default()
                .background(fill.color().scale_alpha(opacity))
                .border(
                    border::rounded(GLYPH / 2.0)
                        .width(1)
                        .color(palette.muted.scale_alpha(opacity)),
                )
        })
        .into(),
    }
}

/// The slim bar: each hint's glyph and word, in the last pad's labels.
pub(crate) fn bar<'a, M: 'a>(
    hints: Vec<(PadButton, &'static str)>,
    style: Style,
    palette: Palette,
) -> Element<'a, M> {
    let items = hints.into_iter().map(|(button, word)| {
        row![
            glyph_view(style, button, palette, 1.0),
            text(word).size(13).color(palette.foreground),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    });
    container(
        Row::with_children(items)
            .spacing(16)
            .align_y(Alignment::Center),
    )
    .padding([0, 12])
    .height(HINT_HEIGHT)
    .center_y(HINT_HEIGHT)
    .style(move |_| crate::library_menu::surface_style(palette))
    .into()
}

#[cfg(test)]
mod tests;
