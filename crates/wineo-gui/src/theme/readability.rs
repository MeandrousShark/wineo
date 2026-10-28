//! Improve readability: nudges failing text roles until they reach 4.5:1.

use iced::Color;

use super::{Palette, PresetKind, contrast};

const REQUIRED: f32 = 4.5;
/// OKLCH lightness moves in steps this small, so the change stays close to
/// the smallest one that passes.
const STEP: f64 = 0.0025;
/// A best-effort round must beat the one before by more than this, so the
/// rounds end.
const MIN_GAIN: f32 = 0.01;
/// A guard; every round gains more than `MIN_GAIN` of a bounded ratio, so the
/// repeats end well before this.
const MAX_ROUNDS: usize = 32;

/// `palette` with each text role that misses 4.5:1 on the background or the
/// surface moved to the nearest lightness, in the same hue, that reaches it.
/// Background, surface, and border are never changed.
pub(super) fn improve(palette: Palette, kind: PresetKind) -> Palette {
    let fix = |role| fix_role(role, palette.background, palette.surface, kind);
    Palette {
        foreground: fix(palette.foreground),
        muted: fix(palette.muted),
        primary: fix(palette.primary),
        success: fix(palette.success),
        warning: fix(palette.warning),
        danger: fix(palette.danger),
        ..palette
    }
}

/// The lower of the contrasts against the two grounds, which decides.
fn lowest_contrast(color: Color, background: Color, surface: Color) -> f32 {
    contrast(color, background).min(contrast(color, surface))
}

fn fix_role(color: Color, background: Color, surface: Color, kind: PresetKind) -> Color {
    let mut current = color;
    for _ in 0..MAX_ROUNDS {
        match search(current, background, surface, kind) {
            Search::Passed(found) => return found,
            // The rounded, gamut-mapped result has its own lightness, chroma,
            // and hue, so searching again from it can still gain. Repeating
            // until nothing gains makes the result a fixed point.
            Search::Improved(better) => current = better,
            Search::Stuck => return current,
        }
    }
    current
}

enum Search {
    /// The color passes, as given or after moving it.
    Passed(Color),
    /// Nothing passes; this is the best color found, better than the start.
    Improved(Color),
    Stuck,
}

fn search(color: Color, background: Color, surface: Color, kind: PresetKind) -> Search {
    let start = lowest_contrast(color, background, surface);
    if start >= REQUIRED {
        return Search::Passed(color);
    }
    let (lightness, chroma, hue) = to_oklch(color);
    let directions = match kind {
        PresetKind::Light => [Direction::Darker, Direction::Lighter],
        PresetKind::Dark => [Direction::Lighter, Direction::Darker],
    };

    let mut nearest: Option<(f64, Color)> = None;
    let mut best = (start, color);
    for direction in directions {
        let room = match direction {
            Direction::Lighter => 1.0 - lightness,
            Direction::Darker => lightness,
        };
        let steps = (room / STEP).ceil() as u32;
        for step in 1..=steps {
            // The last step lands exactly on black or white.
            let moved = (f64::from(step) * STEP).min(room);
            let target = match direction {
                Direction::Lighter => lightness + moved,
                Direction::Darker => lightness - moved,
            };
            let candidate = candidate(target, chroma, hue);
            let ratio = lowest_contrast(candidate, background, surface);
            if ratio >= REQUIRED {
                // Strictly smaller only, so a tie keeps the kind's way.
                if nearest.is_none_or(|(closest, _)| moved < closest) {
                    nearest = Some((moved, candidate));
                }
                break;
            }
            if ratio > best.0 {
                best = (ratio, candidate);
            }
        }
    }

    match nearest {
        Some((_, found)) => Search::Passed(found),
        None if best.0 > start + MIN_GAIN => Search::Improved(best.1),
        None => Search::Stuck,
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Lighter,
    Darker,
}

/// The 8-bit sRGB color at `lightness` and `hue`, with `chroma` reduced until
/// it fits the sRGB gamut. Reducing chroma keeps the hue where clipping
/// channels one by one would shift it.
fn candidate(lightness: f64, chroma: f64, hue: f64) -> Color {
    let fits = |chroma: f64| {
        let rgb = oklch_to_linear(lightness, chroma, hue);
        rgb.iter()
            .all(|&channel| (-1e-4..=1.0 + 1e-4).contains(&channel))
    };
    let chroma = if fits(chroma) {
        chroma
    } else {
        let (mut low, mut high) = (0.0, chroma);
        for _ in 0..32 {
            let middle = (low + high) / 2.0;
            if fits(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        low
    };
    let [r, g, b] = oklch_to_linear(lightness, chroma, hue).map(encode);
    Color::from_rgb8(r, g, b)
}

fn to_oklch(color: Color) -> (f64, f64, f64) {
    let [r, g, b] = [color.r, color.g, color.b].map(|channel| decode(f64::from(channel)));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    (lightness, a.hypot(b), b.atan2(a))
}

/// Linear sRGB, which can fall outside 0 to 1 for colors the display can't show.
fn oklch_to_linear(lightness: f64, chroma: f64, hue: f64) -> [f64; 3] {
    let (a, b) = (chroma * hue.cos(), chroma * hue.sin());
    let l = (lightness + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (lightness - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (lightness - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// An sRGB channel in 0 to 1 as a linear one.
fn decode(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear channel as the nearest 8-bit sRGB value.
fn encode(linear: f64) -> u8 {
    let linear = linear.clamp(0.0, 1.0);
    let value = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0).round() as u8
}
