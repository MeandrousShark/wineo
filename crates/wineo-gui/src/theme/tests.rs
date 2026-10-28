use iced::theme::Mode;

use super::*;

const WINEO_AND_MACCHIATO: [&str; 6] = [
    "midnight",
    "daylight",
    "cellar",
    "cellar-light",
    "high-contrast",
    "catppuccin-macchiato",
];

fn text_roles(palette: Palette) -> [(&'static str, iced::Color); 6] {
    [
        ("foreground", palette.foreground),
        ("muted", palette.muted),
        ("primary", palette.primary),
        ("success", palette.success),
        ("warning", palette.warning),
        ("danger", palette.danger),
    ]
}

/// The text roles that miss 4.5:1 on the background or the surface.
fn failing_roles(palette: Palette) -> Vec<&'static str> {
    text_roles(palette)
        .into_iter()
        .filter(|&(_, color)| {
            contrast(color, palette.background) < 4.5 || contrast(color, palette.surface) < 4.5
        })
        .map(|(role, _)| role)
        .collect()
}

fn preset(id: &str) -> &'static Preset {
    Preset::find(id).unwrap_or_else(|| panic!("no preset {id}"))
}

#[test]
fn the_table_lists_the_eight_presets_with_their_kinds() {
    let listed: Vec<_> = PRESETS
        .iter()
        .map(|preset| (preset.id, preset.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("midnight", PresetKind::Dark),
            ("daylight", PresetKind::Light),
            ("cellar", PresetKind::Dark),
            ("cellar-light", PresetKind::Light),
            ("high-contrast", PresetKind::Dark),
            ("dracula", PresetKind::Dark),
            ("catppuccin-macchiato", PresetKind::Dark),
            ("catppuccin-latte", PresetKind::Light),
        ]
    );
    assert!(Preset::find("nonsense").is_none());
    assert!(Preset::find("custom:midnight").is_none());
}

#[test]
fn midnight_is_the_dark_palette_with_the_fixed_danger() {
    assert_eq!(preset("midnight").palette, Palette::DARK);
    assert_eq!(Palette::DARK.danger, rgb(0xec625a));
}

#[test]
fn from_mode_resolves_the_defaults() {
    assert_eq!(Palette::from_mode(Mode::Dark), preset("midnight").palette);
    assert_eq!(Palette::from_mode(Mode::Light), preset("daylight").palette);
}

#[test]
fn wineo_presets_and_macchiato_pass_text_contrast_on_both_grounds() {
    for id in WINEO_AND_MACCHIATO {
        let failing = failing_roles(preset(id).palette);
        assert!(failing.is_empty(), "{id} fails 4.5:1 for {failing:?}");
    }
}

#[test]
fn dracula_and_latte_ship_as_published_and_miss_somewhere() {
    for id in ["dracula", "catppuccin-latte"] {
        assert!(
            !failing_roles(preset(id).palette).is_empty(),
            "{id} now passes; add it to the passing list"
        );
    }
}

fn appearance(follow_system: bool, dark: &str, light: &str, theme: &str) -> Appearance {
    Appearance {
        follow_system,
        dark: dark.into(),
        light: light.into(),
        theme: theme.into(),
        improve_readability: false,
    }
}

#[test]
fn following_the_system_picks_the_slot_for_the_mode() {
    let appearance = appearance(true, "dracula", "catppuccin-latte", "cellar");
    assert_eq!(
        resolve(&appearance, Mode::Dark, &[]),
        preset("dracula").palette
    );
    assert_eq!(
        resolve(&appearance, Mode::Light, &[]),
        preset("catppuccin-latte").palette
    );
}

#[test]
fn a_fixed_theme_ignores_the_mode() {
    let appearance = appearance(false, "dracula", "catppuccin-latte", "cellar-light");
    for mode in [Mode::Dark, Mode::Light] {
        assert_eq!(
            resolve(&appearance, mode, &[]),
            preset("cellar-light").palette
        );
    }
}

#[test]
fn unknown_and_wrong_kind_ids_fall_back_per_slot() {
    let midnight = preset("midnight").palette;
    let daylight = preset("daylight").palette;
    for bad in ["nonsense", "", "custom:mine"] {
        let appearance = appearance(true, bad, bad, bad);
        assert_eq!(
            resolve(&appearance, Mode::Dark, &[]),
            midnight,
            "{bad:?} dark"
        );
        assert_eq!(
            resolve(&appearance, Mode::Light, &[]),
            daylight,
            "{bad:?} light"
        );
        let fixed = self::appearance(false, bad, bad, bad);
        assert_eq!(resolve(&fixed, Mode::Light, &[]), midnight, "{bad:?} theme");
    }
    // A light theme in the dark slot and the reverse.
    let swapped = appearance(true, "daylight", "midnight", "midnight");
    assert_eq!(resolve(&swapped, Mode::Dark, &[]), midnight);
    assert_eq!(resolve(&swapped, Mode::Light, &[]), daylight);
    // The single theme takes either kind.
    let fixed = appearance(false, "midnight", "daylight", "daylight");
    assert_eq!(resolve(&fixed, Mode::Dark, &[]), daylight);
}
