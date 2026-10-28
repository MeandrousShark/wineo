use super::readability::improve;
use super::{Palette, Preset, PresetKind};

const DEFAULT_DARK: &str = "midnight";
const DEFAULT_LIGHT: &str = "daylight";

/// Which theme the app shows, as the user set it. Ids are strings so custom
/// themes can join the built-in ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Appearance {
    pub(crate) follow_system: bool,
    pub(crate) dark: String,
    pub(crate) light: String,
    /// Used when `follow_system` is off.
    pub(crate) theme: String,
    pub(crate) improve_readability: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            follow_system: true,
            dark: DEFAULT_DARK.into(),
            light: DEFAULT_LIGHT.into(),
            theme: DEFAULT_DARK.into(),
            improve_readability: false,
        }
    }
}

/// The palette for `appearance` under the system `mode`.
pub(crate) fn resolve(appearance: &Appearance, mode: iced::theme::Mode) -> Palette {
    let preset = if !appearance.follow_system {
        pick(&appearance.theme, None, DEFAULT_DARK)
    } else if mode == iced::theme::Mode::Light {
        pick(&appearance.light, Some(PresetKind::Light), DEFAULT_LIGHT)
    } else {
        pick(&appearance.dark, Some(PresetKind::Dark), DEFAULT_DARK)
    };
    if appearance.improve_readability {
        improve(preset.palette, preset.kind)
    } else {
        preset.palette
    }
}

/// The preset `id` names, or `fallback` when it is unknown or not of `kind`.
fn pick(id: &str, kind: Option<PresetKind>, fallback: &str) -> &'static Preset {
    Preset::find(id)
        .filter(|preset| kind.is_none_or(|kind| preset.kind == kind))
        .or_else(|| Preset::find(fallback))
        .expect("the fallback ids are built-in presets")
}
