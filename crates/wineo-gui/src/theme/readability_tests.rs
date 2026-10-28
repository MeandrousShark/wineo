use iced::theme::Mode;

use super::readability::improve;
use super::*;

const ROLES: [&str; 6] = [
    "foreground",
    "muted",
    "primary",
    "success",
    "warning",
    "danger",
];

fn text_roles(palette: Palette) -> [Color; 6] {
    [
        palette.foreground,
        palette.muted,
        palette.primary,
        palette.success,
        palette.warning,
        palette.danger,
    ]
}

fn lowest(color: Color, palette: Palette) -> f32 {
    contrast(color, palette.background).min(contrast(color, palette.surface))
}

fn hex(color: Color) -> u32 {
    let [r, g, b, _] = color.into_rgba8();
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

fn hexes(palette: Palette) -> [u32; 9] {
    [
        palette.background,
        palette.surface,
        palette.border,
        palette.foreground,
        palette.muted,
        palette.primary,
        palette.success,
        palette.warning,
        palette.danger,
    ]
    .map(hex)
}

fn preset(id: &str) -> &'static Preset {
    Preset::find(id).unwrap_or_else(|| panic!("no preset {id}"))
}

fn readable(id: &str) -> Palette {
    let preset = preset(id);
    improve(preset.palette, preset.kind)
}

#[test]
fn every_preset_passes_on_both_grounds_with_readability() {
    for preset in PRESETS {
        let palette = improve(preset.palette, preset.kind);
        for (role, color) in ROLES.into_iter().zip(text_roles(palette)) {
            let ratio = lowest(color, palette);
            assert!(ratio >= 4.5, "{} {role}: {ratio}", preset.id);
        }
    }
}

#[test]
fn passing_presets_come_back_identical() {
    for id in [
        "midnight",
        "daylight",
        "cellar",
        "cellar-light",
        "high-contrast",
        "catppuccin-macchiato",
    ] {
        assert_eq!(readable(id), preset(id).palette, "{id}");
    }
}

#[test]
fn grounds_and_border_are_never_changed() {
    for preset in PRESETS {
        let palette = improve(preset.palette, preset.kind);
        assert_eq!(palette.background, preset.palette.background);
        assert_eq!(palette.surface, preset.palette.surface);
        assert_eq!(palette.border, preset.palette.border);
    }
}

#[test]
fn dracula_output_is_pinned() {
    // The canvas hand-picked a lighter muted; the others already pass.
    assert_eq!(
        hexes(readable("dracula")),
        [
            0x21222c, 0x282a36, 0x44475a, 0xf8f8f2, 0x7f90c4, 0xbd93f9, 0x50fa7b, 0xffb86c,
            0xff5555
        ]
    );
}

#[test]
fn latte_output_is_pinned() {
    // The canvas hand-picked darker greens and yellows.
    assert_eq!(
        hexes(readable("catppuccin-latte")),
        [
            0xe6e9ef, 0xeff1f5, 0xccd0da, 0x4c4f69, 0x65687e, 0x8738ee, 0x1e7900, 0x965c00,
            0xd10d38
        ]
    );
}

/// A small deterministic generator, so the sample never changes.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as u32
    }
    /// A color whose channels each fall in `low..=high`.
    fn color(&mut self, low: u32, high: u32) -> Color {
        let mut channel = || (low + self.next() % (high - low + 1)) as u8;
        Color::from_rgb8(channel(), channel(), channel())
    }
}

/// Palettes with dark, light, and mixed grounds and random roles, each paired
/// with both kinds so a light ground can be labeled Dark and the reverse.
fn sample() -> Vec<(Palette, PresetKind)> {
    let mut random = Lcg(0x0057);
    let mut palettes = vec![];
    for round in 0..900 {
        let (low, high) = match round % 3 {
            0 => (0, 90),
            1 => (165, 255),
            _ => (0, 255),
        };
        let background = random.color(low, high);
        let surface = match round % 5 {
            0 => random.color(0, 255),
            _ => random.color(low, high),
        };
        let palette = Palette {
            background,
            surface,
            border: random.color(0, 255),
            foreground: random.color(0, 255),
            muted: random.color(0, 255),
            primary: random.color(0, 255),
            success: random.color(0, 255),
            warning: random.color(0, 255),
            danger: random.color(0, 255),
        };
        let kind = if random.next().is_multiple_of(2) {
            PresetKind::Dark
        } else {
            PresetKind::Light
        };
        palettes.push((palette, kind));
    }
    // Impossible, and borderline: no 8-bit gray passes against black and
    // #fbfbfb, but #65796c does (about 4.505:1).
    for (background, surface) in [(0x000000, 0x777777), (0x000000, 0xfbfbfb)] {
        for kind in [PresetKind::Dark, PresetKind::Light] {
            let palette = Palette {
                background: rgb(background),
                surface: rgb(surface),
                ..preset("midnight").palette
            };
            palettes.push((palette, kind));
        }
    }
    palettes
}

/// What the grounds allow a text role to reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passing {
    /// Some 8-bit gray passes on both grounds, so a passing color exists.
    Possible,
    /// No luminance reaches 4.5:1 on both grounds.
    Impossible,
    /// A passing color may exist, but no 8-bit gray is one.
    Borderline,
}

fn passing(palette: Palette) -> Passing {
    let gray_passes = (0..=255).any(|value| {
        let gray = Color::from_rgb8(value, value, value);
        lowest(gray, palette) >= 4.5
    });
    if gray_passes {
        return Passing::Possible;
    }
    // Contrast depends only on luminance. Against a ground of luminance `g`,
    // 4.5:1 holds for luminances up to (g + 0.05) / 4.5 - 0.05 and from
    // 4.5 (g + 0.05) - 0.05 up. Impossible means these ranges miss each other
    // for the two grounds.
    let ranges = |ground: Color| {
        let g = f64::from(luminance(ground)) + 0.05;
        [(0.0, g / 4.5 - 0.05), (4.5 * g - 0.05, 1.0)]
    };
    let overlap = ranges(palette.background)
        .into_iter()
        .any(|(low_a, high_a)| {
            ranges(palette.surface)
                .into_iter()
                .any(|(low_b, high_b)| low_a.max(low_b) <= high_a.min(high_b))
        });
    if overlap {
        Passing::Borderline
    } else {
        Passing::Impossible
    }
}

#[test]
fn the_sample_passes_where_possible_and_never_gets_worse() {
    let (mut possible, mut impossible, mut borderline, mut changed) = (0, 0, 0, 0);
    for (palette, kind) in sample() {
        let improved = improve(palette, kind);
        assert_eq!(improved.background, palette.background);
        assert_eq!(improved.surface, palette.surface);
        assert_eq!(improved.border, palette.border);
        let outcome = passing(palette);
        match outcome {
            Passing::Possible => possible += 1,
            Passing::Impossible => impossible += 1,
            Passing::Borderline => borderline += 1,
        }
        for (role, (before, after)) in ROLES
            .into_iter()
            .zip(text_roles(palette).into_iter().zip(text_roles(improved)))
        {
            if before != after {
                changed += 1;
            }
            let (before, after) = (lowest(before, palette), lowest(after, palette));
            if outcome == Passing::Possible {
                assert!(after >= 4.5, "{role} ends at {after} in {palette:?}");
            }
            assert!(after >= before, "{role} fell from {before} to {after}");
        }
    }
    // The sample has to exercise both branches.
    assert!(possible > 300, "only {possible} palettes can pass");
    assert!(impossible > 10, "only {impossible} palettes cannot pass");
    assert!(borderline >= 2, "only {borderline} borderline palettes");
    assert!(changed > 1000, "only {changed} roles changed");
}

#[test]
fn a_best_effort_result_is_a_fixed_point() {
    // A second search from the first result's own rounded OKLCH used to move
    // #3964ff to #3a65ff.
    let repro = Palette {
        background: rgb(0x1b1118),
        surface: rgb(0xfde6e1),
        foreground: rgb(0x1819e4),
        ..preset("midnight").palette
    };
    assert_eq!(passing(repro), Passing::Impossible);
    let once = improve(repro, PresetKind::Light);
    assert_eq!(improve(once, PresetKind::Light), once);
    assert!(lowest(once.foreground, once) > lowest(repro.foreground, repro));

    // Grounds on opposite sides leave a best effort near the middle.
    let grounds = [
        (0x1b1118, 0xfde6e1),
        (0x202020, 0xd8d8d8),
        (0x101820, 0xe0e8f0),
        (0x000000, 0xfbfbfb),
        (0x2a1010, 0xf0f8e8),
        (0x0a0a30, 0xfff0d0),
    ];
    let roles = [
        0x1819e4, 0xe41819, 0x18e419, 0x808080, 0x6d8cff, 0xe5a34a, 0xbd93f9,
    ];
    for (background, surface) in grounds {
        for role in roles {
            let palette = Palette {
                background: rgb(background),
                surface: rgb(surface),
                foreground: rgb(role),
                muted: rgb(role),
                primary: rgb(role),
                success: rgb(role),
                warning: rgb(role),
                danger: rgb(role),
                border: rgb(0x777777),
            };
            for kind in [PresetKind::Dark, PresetKind::Light] {
                let once = improve(palette, kind);
                assert_eq!(
                    improve(once, kind),
                    once,
                    "{background:06x} {surface:06x} {role:06x} {kind:?}"
                );
                assert!(lowest(once.foreground, once) >= lowest(palette.foreground, palette));
            }
        }
    }
}

#[test]
fn black_and_a_mid_gray_leave_white_as_the_best_effort() {
    let palette = Palette {
        background: rgb(0x000000),
        surface: rgb(0x777777),
        ..preset("midnight").palette
    };
    assert_eq!(passing(palette), Passing::Impossible);
    for kind in [PresetKind::Dark, PresetKind::Light] {
        let improved = improve(palette, kind);
        // White gives 4.48:1 on the gray, the most any color can.
        assert_eq!(improved.foreground, rgb(0xffffff), "{kind:?}");
        assert_eq!(improved.danger, rgb(0xffffff), "{kind:?}");
    }
}

#[test]
fn improving_twice_changes_nothing() {
    for preset in PRESETS {
        let once = improve(preset.palette, preset.kind);
        assert_eq!(improve(once, preset.kind), once, "{}", preset.id);
    }
    for (palette, kind) in sample() {
        let once = improve(palette, kind);
        assert_eq!(improve(once, kind), once, "{palette:?} {kind:?}");
    }
}

#[test]
fn the_direction_follows_the_grounds_not_the_label() {
    // Judging by the result keeps a light ground labeled Dark from being
    // pushed lighter, where nothing can pass.
    let light_ground = Palette {
        background: rgb(0xf0f0f0),
        surface: rgb(0xffffff),
        foreground: rgb(0xaaaaaa),
        ..preset("daylight").palette
    };
    for kind in [PresetKind::Dark, PresetKind::Light] {
        let improved = improve(light_ground, kind);
        assert!(luminance(improved.foreground) < luminance(rgb(0xaaaaaa)));
    }
}

#[test]
fn resolve_applies_the_transform_only_when_asked() {
    for preset in PRESETS {
        let mut appearance = Appearance {
            follow_system: false,
            theme: preset.id.into(),
            ..Appearance::default()
        };
        assert_eq!(resolve(&appearance, Mode::Dark), preset.palette);
        appearance.improve_readability = true;
        assert_eq!(
            resolve(&appearance, Mode::Dark),
            improve(preset.palette, preset.kind),
            "{}",
            preset.id
        );
    }
    let following = Appearance {
        dark: "dracula".into(),
        light: "catppuccin-latte".into(),
        improve_readability: true,
        ..Appearance::default()
    };
    assert_eq!(resolve(&following, Mode::Dark), readable("dracula"));
    assert_eq!(
        resolve(&following, Mode::Light),
        readable("catppuccin-latte")
    );
}
