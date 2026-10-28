use iced::{Color, Theme};

mod appearance;
mod presets;
mod readability;
#[cfg(test)]
mod readability_tests;
#[cfg(test)]
mod tests;

pub(crate) use appearance::{Appearance, resolve};
#[cfg(test)]
pub(crate) use presets::PRESETS;
pub(crate) use presets::{Preset, PresetKind};

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// WCAG 2 relative luminance of an opaque color.
pub(crate) fn luminance(color: Color) -> f32 {
    fn linear(value: f32) -> f32 {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// The WCAG contrast ratio between two opaque colors.
pub(crate) fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Palette {
    pub(super) background: Color,
    pub(super) surface: Color,
    pub(super) border: Color,
    pub(super) foreground: Color,
    pub(super) muted: Color,
    pub(super) primary: Color,
    pub(super) success: Color,
    pub(super) warning: Color,
    pub(super) danger: Color,
}

impl Palette {
    /// The colors in role order: background, surface, border, foreground,
    /// muted, primary, success, warning, danger.
    const fn from_hex(hex: [u32; 9]) -> Self {
        Self {
            background: rgb(hex[0]),
            surface: rgb(hex[1]),
            border: rgb(hex[2]),
            foreground: rgb(hex[3]),
            muted: rgb(hex[4]),
            primary: rgb(hex[5]),
            success: rgb(hex[6]),
            warning: rgb(hex[7]),
            danger: rgb(hex[8]),
        }
    }
    pub(super) const DARK: Self = presets::MIDNIGHT;
    #[cfg(test)]
    pub(super) fn from_mode(mode: iced::theme::Mode) -> Self {
        resolve(&Appearance::default(), mode)
    }
    /// Every color at `alpha` times its opacity.
    pub(super) fn faded(self, alpha: f32) -> Self {
        Self {
            background: self.background.scale_alpha(alpha),
            surface: self.surface.scale_alpha(alpha),
            border: self.border.scale_alpha(alpha),
            foreground: self.foreground.scale_alpha(alpha),
            muted: self.muted.scale_alpha(alpha),
            primary: self.primary.scale_alpha(alpha),
            success: self.success.scale_alpha(alpha),
            warning: self.warning.scale_alpha(alpha),
            danger: self.danger.scale_alpha(alpha),
        }
    }
    pub(super) fn theme(self) -> Theme {
        Theme::custom(
            "wineo",
            iced::theme::Palette {
                background: self.background,
                text: self.foreground,
                primary: self.primary,
                success: self.success,
                warning: self.warning,
                danger: self.danger,
            },
        )
    }
}
