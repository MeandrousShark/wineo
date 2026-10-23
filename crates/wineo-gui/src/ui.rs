use std::borrow::Borrow;

use iced::widget::{Button, Column, Container, Space, Text, button, container, scrollable, text};
use iced::{Color, Element, Fill, Font, Theme, border, font};

use crate::theme::Palette;

pub(crate) const BUTTON_HEIGHT: f32 = 32.0;
pub(crate) const CONTROL_RADIUS: f32 = 8.0;
pub(crate) const SECTION_RADIUS: f32 = 12.0;

pub(crate) trait PickListStyle<'a, T, L, V, M>
where
    T: ToString + PartialEq + Clone,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    M: Clone,
{
    fn wineo_style(self, palette: Palette) -> Self;
}

impl<'a, T, L, V, M> PickListStyle<'a, T, L, V, M> for iced::widget::PickList<'a, T, L, V, M>
where
    T: ToString + PartialEq + Clone,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    M: Clone,
{
    fn wineo_style(self, palette: Palette) -> Self {
        self.padding([8, 10])
            .text_size(14)
            .style(move |_, status| pick_list_style(palette, status))
            .menu_style(move |_| pick_menu_style(palette))
    }
}

fn pick_list_style(
    palette: Palette,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;

    let (background, border_color) = match status {
        Status::Active => (palette.surface, palette.border),
        Status::Hovered => (
            blend(palette.surface, palette.foreground, 0.06),
            blend(palette.border, palette.primary, 0.55),
        ),
        Status::Opened { .. } => (
            blend(palette.surface, palette.primary, 0.12),
            palette.primary,
        ),
    };
    iced::widget::pick_list::Style {
        text_color: palette.foreground,
        placeholder_color: palette.muted,
        handle_color: if matches!(status, Status::Opened { .. }) {
            palette.primary
        } else {
            palette.muted
        },
        background: background.into(),
        border: border::rounded(CONTROL_RADIUS).width(1).color(border_color),
    }
}

fn pick_menu_style(palette: Palette) -> iced::overlay::menu::Style {
    iced::overlay::menu::Style {
        background: palette.surface.into(),
        border: border::rounded(CONTROL_RADIUS)
            .width(1)
            .color(palette.border),
        text_color: palette.foreground,
        selected_text_color: accent_ink(palette.primary),
        selected_background: palette.primary.into(),
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.16),
            offset: iced::Vector::new(0.0, 3.0),
            blur_radius: 8.0,
        },
    }
}

/// Thin scrollbar geometry shared by every scrollable in the app.
pub(crate) const SCROLLBAR_WIDTH: f32 = 6.0;
pub(crate) const SCROLLER_WIDTH: f32 = 4.0;

/// Keeps a rail that runs along a dialog's edge clear of its rounded corners.
pub(crate) const EDGE_SCROLL_MARGIN: f32 = 6.0;

pub(crate) trait ScrollStyle {
    /// Vertical scrolling with the shared thin scrollbar, thumb always shown.
    fn wineo_style(self, palette: Palette) -> Self;

    /// The same, for a scrollable flush with a dialog's right edge.
    fn wineo_edge_style(self, palette: Palette) -> Self;
}

fn thin_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(SCROLLBAR_WIDTH)
        .scroller_width(SCROLLER_WIDTH)
}

impl<'a, M: 'a> ScrollStyle for scrollable::Scrollable<'a, M> {
    fn wineo_style(self, palette: Palette) -> Self {
        self.direction(scrollable::Direction::Vertical(thin_scrollbar()))
            .style(move |theme, status| scroll_style(theme, status, palette, true))
    }

    fn wineo_edge_style(self, palette: Palette) -> Self {
        self.direction(scrollable::Direction::Vertical(
            thin_scrollbar().margin(EDGE_SCROLL_MARGIN),
        ))
        .style(move |theme, status| scroll_style(theme, status, palette, true))
    }
}

pub(crate) fn scroll_style(
    theme: &Theme,
    status: scrollable::Status,
    palette: Palette,
    visible: bool,
) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    let show = visible || !matches!(status, scrollable::Status::Active { .. });
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = border::rounded(3);
        rail.scroller.background = if show {
            palette.border.into()
        } else {
            Color::TRANSPARENT.into()
        };
        rail.scroller.border = border::rounded(3);
    }
    style
}

pub(crate) fn secondary_button<'a, M: 'a>(
    label: impl text::IntoFragment<'a>,
    palette: Palette,
) -> Button<'a, M> {
    button(text(label).size(14).center())
        .height(BUTTON_HEIGHT)
        .padding([0, 12])
        .style(move |_, status| secondary_style(palette, status))
}

pub(crate) fn accent_button<M: Clone + 'static>(
    label: impl text::IntoFragment<'static>,
    color: Color,
    enabled: bool,
    message: M,
) -> Element<'static, M> {
    accent_control(label, color, enabled, message).into()
}

pub(crate) fn accent_control<M: Clone + 'static>(
    label: impl text::IntoFragment<'static>,
    color: Color,
    enabled: bool,
    message: M,
) -> Button<'static, M> {
    accent_base(label, color).on_press_maybe(enabled.then_some(message))
}

/// An accent button without a press handler, for callers that decide
/// availability themselves with `on_press` or `on_press_maybe`.
pub(crate) fn accent_base<'a, M: 'a>(
    label: impl text::IntoFragment<'a>,
    color: Color,
) -> Button<'a, M> {
    button(text(label).size(14).center())
        .height(BUTTON_HEIGHT)
        .padding([0, 16])
        .style(move |theme: &iced::Theme, status| accent_style(color, theme.palette().text, status))
}

/// A quiet full-width row for choosing one of several options. Group them
/// with `list_group`.
pub(crate) fn list_row<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    palette: Palette,
) -> Button<'a, M> {
    button(content)
        .width(Fill)
        .padding([10, 12])
        .style(move |_, status| list_row_style(palette, status))
}

pub(crate) fn list_row_text<'a>(label: impl text::IntoFragment<'a>) -> Text<'a> {
    text(label).size(14)
}

pub(crate) fn list_row_style(palette: Palette, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active | button::Status::Disabled => Color::TRANSPARENT,
        button::Status::Hovered => palette.primary.scale_alpha(0.10),
        button::Status::Pressed => palette.primary.scale_alpha(0.22),
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            palette.muted
        } else {
            palette.foreground
        },
        border: border::rounded(CONTROL_RADIUS).width(1).color(
            if status == button::Status::Pressed {
                palette.primary
            } else {
                Color::TRANSPARENT
            },
        ),
        ..button::Style::default()
    }
}

/// A `list_row` for `list_group`, chosen by `on_press` from the mouse or,
/// once focused, from Enter or Space. `id` names the row's choice, like a
/// path or a build id, so focus can follow it when the list changes.
pub(crate) struct ListRow<'a, M> {
    id: String,
    row: Button<'a, M>,
    on_press: M,
    enabled: bool,
}

impl<'a, M: Clone + 'a> ListRow<'a, M> {
    pub(crate) fn new(id: impl Into<String>, row: Button<'a, M>, on_press: M) -> Self {
        Self {
            id: id.into(),
            row,
            on_press,
            enabled: true,
        }
    }

    /// A row that can't be chosen right now takes no press and no focus. No
    /// list disables a row yet.
    #[cfg(test)]
    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Rows in one grouped surface with a separator between each pair. The rows
/// take keyboard focus, and Up and Down move it between them. `list` names
/// the list, as the page's `ListRows` does.
pub(crate) fn list_group<'a, M: Clone + 'a>(
    list: &'static str,
    rows: impl IntoIterator<Item = ListRow<'a, M>>,
    palette: Palette,
) -> Element<'a, M> {
    let mut column = Column::new().width(Fill);
    let mut ids = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            column = column.push(separator(palette));
        }
        let press = row.enabled.then(|| row.on_press.clone());
        column = column.push(crate::focus_button::row(
            row.row.on_press_maybe(press).into(),
            row.enabled,
            row.id.clone(),
            row.on_press,
        ));
        ids.push((row.id, row.enabled));
    }
    crate::focus_button::rows(
        list,
        container(column)
            .width(Fill)
            .padding(4)
            .style(move |_| section_style(palette))
            .into(),
        ids,
    )
}

pub(crate) fn secondary_style(palette: Palette, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active => palette.surface,
        button::Status::Hovered => blend(palette.surface, palette.foreground, 0.08),
        button::Status::Pressed => blend(palette.surface, palette.primary, 0.18),
        button::Status::Disabled => palette.surface,
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            palette.muted
        } else {
            palette.foreground
        },
        border: border::rounded(CONTROL_RADIUS).width(1).color(
            if status == button::Status::Pressed {
                palette.primary
            } else {
                palette.border
            },
        ),
        ..button::Style::default()
    }
}

pub(crate) fn accent_style(
    color: Color,
    foreground: Color,
    status: button::Status,
) -> button::Style {
    let background = match status {
        button::Status::Active => color,
        button::Status::Hovered => blend(color, Color::WHITE, 0.10),
        button::Status::Pressed => blend(color, Color::BLACK, 0.18),
        button::Status::Disabled => color.scale_alpha(0.22),
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            foreground.scale_alpha(0.85)
        } else {
            accent_ink(background)
        },
        border: border::rounded(CONTROL_RADIUS),
        ..button::Style::default()
    }
}

fn luminance(color: Color) -> f32 {
    fn linear(value: f32) -> f32 {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

pub(crate) fn accent_ink(background: Color) -> Color {
    if luminance(background) > 0.179 {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

pub(crate) fn blend(color: Color, toward: Color, amount: f32) -> Color {
    Color {
        r: color.r + (toward.r - color.r) * amount,
        g: color.g + (toward.g - color.g) * amount,
        b: color.b + (toward.b - color.b) * amount,
        a: color.a,
    }
}

pub(crate) fn section<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    palette: Palette,
) -> Container<'a, M> {
    container(content)
        .width(Fill)
        .padding(16)
        .style(move |_| section_style(palette))
}

pub(crate) fn section_style(palette: Palette) -> container::Style {
    container::Style::default()
        .background(palette.surface)
        .border(
            border::rounded(SECTION_RADIUS)
                .width(1)
                .color(palette.border),
        )
}

pub(crate) fn page_title(label: &str, palette: Palette) -> Text<'_> {
    heading(label, palette, 22)
}

pub(crate) fn section_heading(label: &str, palette: Palette) -> Text<'_> {
    heading(label, palette, 16)
}

fn heading(label: &str, palette: Palette, size: u32) -> Text<'_> {
    text(label)
        .size(size)
        .font(Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        })
        .color(palette.foreground)
}

pub(crate) fn separator<'a, M: 'a>(palette: Palette) -> Container<'a, M> {
    container(Space::new())
        .width(Fill)
        .height(1)
        .style(move |_| container::Style::default().background(palette.border))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_labels_remain_readable_in_both_palettes() {
        for mode in [iced::theme::Mode::Dark, iced::theme::Mode::Light] {
            let palette = Palette::from_mode(mode);
            for color in [palette.primary, palette.danger, palette.success] {
                for status in [
                    button::Status::Active,
                    button::Status::Hovered,
                    button::Status::Pressed,
                    button::Status::Disabled,
                ] {
                    let style = accent_style(color, palette.foreground, status);
                    let Some(iced::Background::Color(background)) = style.background else {
                        panic!("expected solid accent fill");
                    };
                    let background = composite(background, palette.surface);
                    let foreground = composite(style.text_color, background);
                    let a = luminance(background);
                    let b = luminance(foreground);
                    let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(contrast >= 4.5, "accent label contrast: {contrast}");
                }
            }
        }
    }

    #[test]
    fn scroll_thumb_shows_when_visible_or_active_and_hides_when_idle() {
        let palette = Palette::from_mode(iced::theme::Mode::Dark);
        let theme = Theme::Dark;
        let idle = scrollable::Status::Active {
            is_horizontal_scrollbar_disabled: false,
            is_vertical_scrollbar_disabled: false,
        };
        let thumb = |status, visible| {
            scroll_style(&theme, status, palette, visible)
                .vertical_rail
                .scroller
                .background
        };
        assert_eq!(thumb(idle, true), palette.border.into());
        assert_eq!(thumb(idle, false), Color::TRANSPARENT.into());
        let hovered = scrollable::Status::Hovered {
            is_horizontal_scrollbar_hovered: false,
            is_vertical_scrollbar_hovered: true,
            is_horizontal_scrollbar_disabled: false,
            is_vertical_scrollbar_disabled: false,
        };
        assert_eq!(thumb(hovered, false), palette.border.into());
    }

    fn composite(color: Color, background: Color) -> Color {
        blend(background, color, color.a)
    }
}
