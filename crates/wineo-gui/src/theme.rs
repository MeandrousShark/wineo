use iced::{Color, Theme};

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[derive(Clone, Copy)]
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
    pub(super) const DARK: Self = Self {
        background: rgb(0x16171b),
        surface: rgb(0x1f2126),
        border: rgb(0x2c2f36),
        foreground: rgb(0xe6e6e9),
        muted: rgb(0x8b8f98),
        primary: rgb(0x6d8cff),
        success: rgb(0x3ecf6e),
        warning: rgb(0xe5a34a),
        danger: rgb(0xe5534b),
    };
    const LIGHT: Self = Self {
        background: rgb(0xf6f7f9),
        surface: rgb(0xffffff),
        border: rgb(0xd9dde4),
        foreground: rgb(0x20242b),
        muted: rgb(0x59616d),
        primary: rgb(0x3158ca),
        success: rgb(0x198044),
        warning: rgb(0xa65d12),
        danger: rgb(0xb93431),
    };
    pub(super) fn from_mode(mode: iced::theme::Mode) -> Self {
        if mode == iced::theme::Mode::Light {
            Self::LIGHT
        } else {
            Self::DARK
        }
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
