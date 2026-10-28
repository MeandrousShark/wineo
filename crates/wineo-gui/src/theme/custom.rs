//! Custom themes as the app sees them: a theme file from core, with every
//! gap filled from the built-in it's based on.

use iced::Color;
use wineo_core::themes::{CUSTOM_PREFIX, Colors, ThemeFile, ThemeKind, Themes};

use super::{Palette, Preset, PresetKind};

/// A custom theme. Its colors stay as the file has them, so an edit writes
/// back only what changed; `palette` fills the gaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Custom {
    /// The id as `library.toml` spells it, `custom:<file id>`.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: PresetKind,
    /// A built-in id, or None for the default of the kind.
    pub(crate) based_on: Option<String>,
    pub(crate) colors: Colors,
    /// A newer wineo wrote it, so core refuses to edit it.
    pub(crate) read_only: bool,
}

impl Custom {
    pub(crate) fn from_file(file: &ThemeFile) -> Self {
        Self {
            id: format!("{CUSTOM_PREFIX}{}", file.id),
            name: file.name.clone(),
            kind: match file.kind {
                Some(ThemeKind::Light) => PresetKind::Light,
                Some(ThemeKind::Dark) | None => PresetKind::Dark,
            },
            based_on: file.based_on.clone(),
            colors: file.colors.clone(),
            read_only: file.newer_version,
        }
    }
    /// The file id, without `custom:`.
    pub(crate) fn file_id(&self) -> &str {
        self.id.strip_prefix(CUSTOM_PREFIX).unwrap_or(&self.id)
    }
    /// The built-in that fills missing colors.
    pub(crate) fn base(&self) -> &'static Preset {
        self.based_on
            .as_deref()
            .and_then(Preset::find)
            .unwrap_or_else(|| super::appearance::default_preset(Some(self.kind)))
    }
    pub(crate) fn palette(&self) -> Palette {
        let base = self.base().palette;
        let mut palette = base;
        for (index, (_, color)) in self.colors.roles().into_iter().enumerate() {
            if let Some(color) = color.and_then(parse_hex) {
                *palette.role_mut(index) = color;
            }
        }
        palette
    }
}

/// Every readable theme file, in `themes::list`'s order. Files that don't
/// parse are left out, so anything pointing at one falls back.
pub(crate) fn customs(themes: &Themes) -> Vec<Custom> {
    themes.themes.iter().map(Custom::from_file).collect()
}

/// `#rrggbb` as a color, or None for anything else.
fn parse_hex(text: &str) -> Option<Color> {
    let normal = wineo_core::themes::normalize_color(text)?;
    let value = u32::from_str_radix(&normal[1..], 16).ok()?;
    Some(super::rgb(value))
}

/// An opaque color as `#rrggbb`.
pub(crate) fn hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// All nine roles of `palette` as core's colors.
pub(crate) fn colors_of(palette: Palette) -> Colors {
    let hex = |index| Some(hex(palette.role(index)));
    Colors {
        background: hex(0),
        surface: hex(1),
        border: hex(2),
        foreground: hex(3),
        muted: hex(4),
        primary: hex(5),
        success: hex(6),
        warning: hex(7),
        danger: hex(8),
    }
}

/// The role `index`, in `ROLES` order, of `colors`.
pub(crate) fn color_mut(colors: &mut Colors, index: usize) -> &mut Option<String> {
    match index {
        0 => &mut colors.background,
        1 => &mut colors.surface,
        2 => &mut colors.border,
        3 => &mut colors.foreground,
        4 => &mut colors.muted,
        5 => &mut colors.primary,
        6 => &mut colors.success,
        7 => &mut colors.warning,
        _ => &mut colors.danger,
    }
}
