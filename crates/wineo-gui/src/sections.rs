//! The library's sections: what Group by picks, and the header row each
//! section shows above its cards.

use iced::widget::{container, row, text};
use iced::{Center, Element, Fill, Font, font};
use wineo_core::library::GroupBy;

use crate::grid_motion::{HEADER_HEIGHT, Header};
use crate::library_menu::{CHEVRON, icon};
use crate::theme::Palette;
use crate::{App, Message};

/// The chevron of a collapsed section, pointing at its name.
pub(crate) const CHEVRON_RIGHT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

/// The Group by choices, in the Sort menu's order.
pub(crate) const GROUPS: [GroupBy; 4] =
    [GroupBy::None, GroupBy::Tag, GroupBy::Source, GroupBy::State];

pub(crate) fn label(group: GroupBy) -> &'static str {
    match group {
        GroupBy::None => "None",
        GroupBy::Tag => "Tag",
        GroupBy::Source => "Source",
        GroupBy::State => "State",
    }
}

/// A header's look: the chevron, then the section's name and its count,
/// across the row at `opacity`.
pub(crate) fn header_content(
    header: &Header,
    palette: Palette,
    opacity: f32,
) -> Element<'static, Message> {
    let palette = palette.faded(opacity);
    let chevron = if header.collapsed {
        CHEVRON_RIGHT
    } else {
        CHEVRON
    };
    container(
        row![
            icon(chevron, palette.muted, 16.0),
            text(header.key.title())
                .size(16)
                .font(Font {
                    weight: font::Weight::Semibold,
                    ..Font::DEFAULT
                })
                .color(palette.foreground)
                .wrapping(text::Wrapping::None),
            text(format!("({})", header.count))
                .size(16)
                .color(palette.muted)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(Center),
    )
    .width(Fill)
    .height(HEADER_HEIGHT)
    .padding([0, 4])
    .center_y(HEADER_HEIGHT)
    .clip(true)
    .id(header.key.header_id())
    .into()
}

impl App {
    /// The header for `header` in the grid or the list, at `opacity`.
    pub(crate) fn section_header(&self, header: &Header, opacity: f32) -> Element<'_, Message> {
        header_content(header, self.palette, opacity * self.collapse.alpha())
    }
}

#[cfg(test)]
mod tests;
