//! The page frame every import flow shares: width, padding, scrolling, and the pinned footer.

use iced::widget::{container, scrollable};
use iced::{Element, Fill, border};

use crate::theme::Palette;
use crate::ui::ScrollStyle;

/// The widest an import page's content grows in a wide dialog.
pub(super) const PAGE_MAX_WIDTH: f32 = 780.0;
pub(super) const PAGE_PADDING: f32 = 24.0;
/// Keeps scrolled text clear of the scrollbar, which draws over its content.
pub(super) const SCROLL_GUTTER: f32 = crate::ui::SCROLLBAR_WIDTH + 6.0;

/// Insets an import page from the dialog's rounded border on every side, so
/// Back sits inside the corner and scrolled content ends above the bottom.
pub(super) fn page_frame<'a, Message: 'a>(
    page: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(page)
        .padding(PAGE_PADDING)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .into()
}

/// A step's action row, pinned under the scrolling content.
pub(super) fn pinned<'a, Message: 'a>(
    footer: impl Into<Element<'a, Message>>,
    palette: Palette,
) -> Element<'a, Message> {
    container(footer)
        .padding(12)
        .width(Fill)
        .style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(8).width(1).color(palette.border))
        })
        .into()
}

pub(super) fn scroll_body<'a, Message: 'a>(
    body: impl Into<Element<'a, Message>>,
    id: Option<&'static str>,
    palette: Palette,
) -> Element<'a, Message> {
    let mut body = scrollable(container(body).padding(iced::Padding::ZERO.right(SCROLL_GUTTER)))
        .wineo_style(palette)
        .width(Fill)
        .height(Fill);
    if let Some(id) = id {
        body = body.id(iced::widget::Id::new(id));
    }
    body.into()
}
