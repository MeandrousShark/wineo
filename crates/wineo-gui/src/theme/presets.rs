use super::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresetKind {
    Dark,
    Light,
}

/// A built-in theme.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Preset {
    /// The spelling used in config.
    pub(crate) id: &'static str,
    #[expect(dead_code, reason = "the Appearance tab lists it")]
    pub(crate) name: &'static str,
    pub(crate) kind: PresetKind,
    #[expect(dead_code, reason = "the Appearance tab lists it")]
    pub(crate) description: &'static str,
    pub(crate) palette: Palette,
    /// Where the colors come from.
    #[expect(dead_code, reason = "the notices and the Appearance tab cite it")]
    pub(crate) source: &'static str,
}

pub(super) const MIDNIGHT: Palette = Palette::from_hex([
    0x16171b, 0x1f2126, 0x2c2f36, 0xe6e6e9, 0x8b8f98, 0x6d8cff, 0x3ecf6e, 0xe5a34a, 0xec625a,
]);

pub(crate) const PRESETS: [Preset; 8] = [
    Preset {
        id: "midnight",
        name: "Midnight",
        kind: PresetKind::Dark,
        description: "wineo's dark theme, with a blue accent.",
        palette: MIDNIGHT,
        source: "wineo",
    },
    Preset {
        id: "daylight",
        name: "Daylight",
        kind: PresetKind::Light,
        description: "wineo's light theme, with a blue accent.",
        palette: Palette::from_hex([
            0xf6f7f9, 0xffffff, 0xd9dde4, 0x20242b, 0x59616d, 0x3158ca, 0x198044, 0xa65d12,
            0xb93431,
        ]),
        source: "wineo",
    },
    Preset {
        id: "cellar",
        name: "Cellar",
        kind: PresetKind::Dark,
        description: "Warm dark browns with a coral accent.",
        palette: Palette::from_hex([
            0x191513, 0x231d1a, 0x3a302a, 0xeee6df, 0xa8988c, 0xe08060, 0x5fcb8c, 0xe9c25a,
            0xf26d6d,
        ]),
        source: "wineo, accent from Ember",
    },
    Preset {
        id: "cellar-light",
        name: "Cellar Light",
        kind: PresetKind::Light,
        description: "Warm paper tones with a coral accent.",
        palette: Palette::from_hex([
            0xf3ede6, 0xfbf8f4, 0xddd2c6, 0x2a211b, 0x6b5b4f, 0xb44a2f, 0x2f7a45, 0x8f6400,
            0xb0233f,
        ]),
        source: "wineo, accent from Ember's light coral",
    },
    Preset {
        id: "high-contrast",
        name: "High contrast",
        kind: PresetKind::Dark,
        description: "Black ground, bright text, visible borders.",
        palette: Palette::from_hex([
            0x000000, 0x0e0f11, 0x7a7f8a, 0xffffff, 0xc9ccd3, 0x94b4ff, 0x62e48f, 0xffc46a,
            0xff8277,
        ]),
        source: "wineo",
    },
    Preset {
        id: "dracula",
        name: "Dracula",
        kind: PresetKind::Dark,
        description: "The Dracula palette.",
        palette: Palette::from_hex([
            0x21222c, 0x282a36, 0x44475a, 0xf8f8f2, 0x6272a4, 0xbd93f9, 0x50fa7b, 0xffb86c,
            0xff5555,
        ]),
        source: "Dracula spec",
    },
    Preset {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        kind: PresetKind::Dark,
        description: "Catppuccin's Macchiato flavor.",
        palette: Palette::from_hex([
            0x1e2030, 0x24273a, 0x363a4f, 0xcad3f5, 0xa5adcb, 0xc6a0f6, 0xa6da95, 0xeed49f,
            0xed8796,
        ]),
        source: "Catppuccin palette",
    },
    Preset {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        kind: PresetKind::Light,
        description: "Catppuccin's Latte flavor.",
        palette: Palette::from_hex([
            0xe6e9ef, 0xeff1f5, 0xccd0da, 0x4c4f69, 0x6c6f85, 0x8839ef, 0x40a02b, 0xdf8e1d,
            0xd20f39,
        ]),
        source: "Catppuccin palette",
    },
];

impl Preset {
    pub(crate) fn find(id: &str) -> Option<&'static Preset> {
        PRESETS.iter().find(|preset| preset.id == id)
    }
}
