//! The library's sections: what Group by picks, and the header row each
//! section shows above its cards.

use iced::widget::{button, container, row, text};
use std::collections::BTreeSet;

use iced::{Center, Element, Fill, Font, Task, font};
use wineo_core::library::GroupBy;

use crate::grid_motion::{HEADER_HEIGHT, Header};
use crate::library_menu::{CHEVRON, icon};
use crate::slot::SectionKey;
use crate::theme::Palette;
use crate::{App, Message, focus_button, ui};

/// The chevron of a collapsed section, pointing at its name.
pub(crate) const CHEVRON_RIGHT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

/// The Group by choices, in the Sort menu's order.
pub(crate) const GROUPS: [GroupBy; 5] = [
    GroupBy::None,
    GroupBy::Recent,
    GroupBy::Tag,
    GroupBy::Source,
    GroupBy::State,
];

pub(crate) fn label(group: GroupBy) -> &'static str {
    match group {
        GroupBy::None => "None",
        GroupBy::Recent => "Recently played",
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
    /// The header for `header` in the grid or the list, at `opacity`. A
    /// shown one is a quiet row that a click, or Enter or Space while it
    /// has focus, collapses or opens; Tab reaches it, but a click doesn't
    /// focus it, as on the layout switch. A leaving one is only drawn.
    pub(crate) fn section_header(
        &self,
        header: &Header,
        opacity: f32,
        shown: bool,
    ) -> Element<'_, Message> {
        let content = header_content(header, self.palette, opacity * self.collapse.alpha());
        if !shown {
            return content;
        }
        let palette = self.palette;
        let toggle = Message::ToggleSection(header.key.clone());
        let row = button(content)
            .width(Fill)
            .height(HEADER_HEIGHT)
            .padding(0)
            .on_press(toggle.clone())
            .style(move |_, status| ui::list_row_style(palette, status));
        focus_button::keyboard_control(row.into(), true, header.key.header_id(), toggle)
    }

    /// Collapses the section `key`, or opens it again. Only what's left in
    /// view counts as shown (plan 0062, question 22), so marks on games
    /// with no copy left in view drop, and a cursor inside moves to the
    /// header while the panel stays on its game.
    pub(crate) fn toggle_section(&mut self, key: SectionKey) -> Task<Message> {
        let cursor_inside = self
            .selected_slot()
            .is_some_and(|slot| slot.section() == &key);
        let collapsing = self.collapsed.insert(key.clone());
        if !collapsing {
            self.collapsed.remove(&key);
        }
        self.reorder();
        self.reflow_grid();
        if collapsing && cursor_inside {
            self.cursor = Some(crate::app_selection::Cursor::Header(key));
        }
        self.load_visible_art()
    }

    /// The games with a copy in view, which is every shown game but those
    /// whose every copy sits in a collapsed section.
    pub(crate) fn visible_names(&self) -> BTreeSet<String> {
        self.grid_places()
            .ids
            .iter()
            .map(|slot| slot.name().to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests;
