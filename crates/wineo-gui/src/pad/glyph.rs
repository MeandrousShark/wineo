//! Button glyphs and the hint bar. Face buttons are drawn SVGs, since Noto
//! Sans lacks the PlayStation shapes; the rest are short labels in a pill.

use iced::widget::{Row, container, row, text};
use iced::{Alignment, Element, border};
use wineo_core::library::{Bindings, PadAction, PadButton};

use super::Style;
use crate::theme::Palette;

/// The bar's height, which the grid keeps clear under its last row.
pub(crate) const HINT_HEIGHT: f32 = 32.0;
const GLYPH: f32 = 20.0;

// Each is a ring with its mark inside, stroked in black for `svg::Style` to
// tint.
const LETTER_A: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 16.5L12 7.5l3.5 9M9.7 13.5h4.6"/></g></svg>"##;
const LETTER_B: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 7.5v9h3.8a2.3 2.3 0 0 0 0-4.6H9M9 7.5h3.3a2.2 2.2 0 0 1 0 4.4"/></g></svg>"##;
const LETTER_X: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8.8 7.8l6.4 8.4M15.2 7.8l-6.4 8.4"/></g></svg>"##;
const LETTER_Y: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8.8 7.8L12 12l3.2-4.2M12 12v4.5"/></g></svg>"##;
const CROSS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 8.5l7 7M15.5 8.5l-7 7"/></g></svg>"##;
const CIRCLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/></g></svg>"##;
const SQUARE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="8" width="8" height="8"/></g></svg>"##;
const TRIANGLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10.25" fill="none" stroke="#000" stroke-width="1.5"/><g fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 7.8l4.4 7.7H7.6z"/></g></svg>"##;

/// The SVG for a face button, or the text for any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Glyph {
    Face(&'static [u8]),
    Label(&'static str),
}

/// How a pad of `style` shows `button`. `button` is the printed label in
/// Xbox terms, so a Nintendo A and an Xbox A both read A, and a
/// PlayStation pad's cross is its A.
pub(crate) fn glyph(style: Style, button: PadButton) -> Glyph {
    let playstation = style == Style::PlayStation;
    let nintendo = style == Style::Nintendo;
    match button {
        PadButton::A if playstation => Glyph::Face(CROSS),
        PadButton::B if playstation => Glyph::Face(CIRCLE),
        PadButton::X if playstation => Glyph::Face(SQUARE),
        PadButton::Y if playstation => Glyph::Face(TRIANGLE),
        PadButton::A => Glyph::Face(LETTER_A),
        PadButton::B => Glyph::Face(LETTER_B),
        PadButton::X => Glyph::Face(LETTER_X),
        PadButton::Y => Glyph::Face(LETTER_Y),
        PadButton::Lb if playstation => Glyph::Label("L1"),
        PadButton::Rb if playstation => Glyph::Label("R1"),
        PadButton::Lt if playstation => Glyph::Label("L2"),
        PadButton::Rt if playstation => Glyph::Label("R2"),
        PadButton::View if playstation => Glyph::Label("Create"),
        PadButton::Start if playstation => Glyph::Label("Options"),
        PadButton::Ls if playstation => Glyph::Label("L3"),
        PadButton::Rs if playstation => Glyph::Label("R3"),
        PadButton::Lb if nintendo => Glyph::Label("L"),
        PadButton::Rb if nintendo => Glyph::Label("R"),
        PadButton::Lt if nintendo => Glyph::Label("ZL"),
        PadButton::Rt if nintendo => Glyph::Label("ZR"),
        PadButton::View if nintendo => Glyph::Label("-"),
        PadButton::Start if nintendo => Glyph::Label("+"),
        PadButton::Lb => Glyph::Label("LB"),
        PadButton::Rb => Glyph::Label("RB"),
        PadButton::Lt => Glyph::Label("LT"),
        PadButton::Rt => Glyph::Label("RT"),
        PadButton::View => Glyph::Label("View"),
        PadButton::Start => Glyph::Label("Menu"),
        PadButton::Ls => Glyph::Label("LS"),
        PadButton::Rs => Glyph::Label("RS"),
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

fn glyph_view<'a, M: 'a>(style: Style, button: PadButton, palette: Palette) -> Element<'a, M> {
    match glyph(style, button) {
        Glyph::Face(mark) => crate::library_menu::icon(mark, palette.foreground, GLYPH).into(),
        Glyph::Label(label) => container(
            text(label)
                .size(11)
                .font(crate::fonts::semibold())
                .color(palette.foreground),
        )
        .padding([1, 5])
        .style(move |_| {
            container::Style::default().border(
                border::rounded(GLYPH / 2.0)
                    .width(1.5)
                    .color(palette.foreground),
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
            glyph_view(style, button, palette),
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
