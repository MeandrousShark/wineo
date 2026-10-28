use super::readability::improve;
use super::{Palette, Preset, PresetKind};

const DEFAULT_DARK: &str = "midnight";
const DEFAULT_LIGHT: &str = "daylight";

/// Which theme the app shows, as the user set it. Core stores it in
/// `library.toml`; ids are strings so custom themes can join the built-in ones.
pub(crate) type Appearance = wineo_core::library::Appearance;

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

/// The built-in preset a slot shows for `id`, which is the default for the
/// slot when `id` is unknown or not of `kind`, as `resolve` picks it.
pub(crate) fn shown(id: &str, kind: Option<PresetKind>) -> &'static Preset {
    let fallback = if kind == Some(PresetKind::Light) {
        DEFAULT_LIGHT
    } else {
        DEFAULT_DARK
    };
    pick(id, kind, fallback)
}

/// The preset `id` names, or `fallback` when it is unknown or not of `kind`.
fn pick(id: &str, kind: Option<PresetKind>, fallback: &str) -> &'static Preset {
    Preset::find(id)
        .filter(|preset| kind.is_none_or(|kind| preset.kind == kind))
        .or_else(|| Preset::find(fallback))
        .expect("the fallback ids are built-in presets")
}
