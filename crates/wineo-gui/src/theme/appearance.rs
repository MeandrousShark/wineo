use super::readability::improve;
use super::{Custom, Palette, Preset, PresetKind};

const DEFAULT_DARK: &str = "midnight";
const DEFAULT_LIGHT: &str = "daylight";

/// Which theme the app shows, as the user set it. Core stores it in
/// `library.toml`; ids are strings so custom themes can join the built-in ones.
pub(crate) type Appearance = wineo_core::library::Appearance;

/// A theme a slot can show: a built-in preset or a custom theme.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Shown<'a> {
    Preset(&'static Preset),
    Custom(&'a Custom),
}

impl Shown<'_> {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Preset(preset) => preset.id,
            Self::Custom(custom) => &custom.id,
        }
    }
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Preset(preset) => preset.name,
            Self::Custom(custom) => &custom.name,
        }
    }
    pub(crate) fn kind(&self) -> PresetKind {
        match self {
            Self::Preset(preset) => preset.kind,
            Self::Custom(custom) => custom.kind,
        }
    }
    pub(crate) fn palette(&self) -> Palette {
        match self {
            Self::Preset(preset) => preset.palette,
            Self::Custom(custom) => custom.palette(),
        }
    }
}

/// The palette for `appearance` under the system `mode`, with `customs` the
/// custom themes a `custom:` id can name.
pub(crate) fn resolve(
    appearance: &Appearance,
    mode: iced::theme::Mode,
    customs: &[Custom],
) -> Palette {
    let theme = if !appearance.follow_system {
        shown(&appearance.theme, None, customs)
    } else if mode == iced::theme::Mode::Light {
        shown(&appearance.light, Some(PresetKind::Light), customs)
    } else {
        shown(&appearance.dark, Some(PresetKind::Dark), customs)
    };
    if appearance.improve_readability {
        improve(theme.palette(), theme.kind())
    } else {
        theme.palette()
    }
}

/// The theme a slot shows for `id`, which is the default for the slot when
/// `id` is unknown or not of `kind`, as `resolve` picks it.
pub(crate) fn shown<'a>(id: &str, kind: Option<PresetKind>, customs: &'a [Custom]) -> Shown<'a> {
    let fits = |found: PresetKind| kind.is_none_or(|kind| found == kind);
    if let Some(preset) = Preset::find(id).filter(|preset| fits(preset.kind)) {
        return Shown::Preset(preset);
    }
    if let Some(custom) = customs
        .iter()
        .find(|custom| custom.id == id && fits(custom.kind))
    {
        return Shown::Custom(custom);
    }
    Shown::Preset(default_preset(kind))
}

/// The default theme for a slot of `kind`; the single theme's is Midnight.
pub(crate) fn default_preset(kind: Option<PresetKind>) -> &'static Preset {
    let id = if kind == Some(PresetKind::Light) {
        DEFAULT_LIGHT
    } else {
        DEFAULT_DARK
    };
    Preset::find(id).expect("the default ids are built-in presets")
}
