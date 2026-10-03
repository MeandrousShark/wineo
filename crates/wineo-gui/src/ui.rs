use std::borrow::Borrow;

use iced::widget::{Button, Container, Space, Text, button, container, text};
use iced::{Color, Element, Fill, Font, border, font};

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

pub(crate) fn secondary_button<'a, M: 'a>(label: &'a str, palette: Palette) -> Button<'a, M> {
    button(text(label).size(14).center())
        .height(BUTTON_HEIGHT)
        .padding([0, 12])
        .style(move |_, status| secondary_style(palette, status))
}

pub(crate) fn accent_button<M: Clone + 'static>(
    label: &'static str,
    color: Color,
    enabled: bool,
    message: M,
) -> Element<'static, M> {
    accent_control(label, color, enabled, message).into()
}

pub(crate) fn accent_control<M: Clone + 'static>(
    label: &'static str,
    color: Color,
    enabled: bool,
    message: M,
) -> Button<'static, M> {
    button(text(label).size(14).center())
        .height(BUTTON_HEIGHT)
        .padding([0, 16])
        .on_press_maybe(enabled.then_some(message))
        .style(move |theme: &iced::Theme, status| accent_style(color, theme.palette().text, status))
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

fn accent_ink(background: Color) -> Color {
    if luminance(background) > 0.179 {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

fn blend(color: Color, toward: Color, amount: f32) -> Color {
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

    fn composite(color: Color, background: Color) -> Color {
        blend(background, color, color.a)
    }
}
