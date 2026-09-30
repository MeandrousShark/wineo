use std::f32::consts::TAU;
use std::time::Instant;

use iced::widget::svg;
use iced::{Color, Element, Radians, Rotation};

use crate::theme::Palette;

/// A faint track with a rotating three-quarter arc.
pub(crate) fn spinner<'a, Message: 'a>(
    palette: Palette,
    angle: f32,
    size: f32,
) -> Element<'a, Message> {
    // Same bytes for the same palette, so iced reuses the rasterized image
    // and each frame only changes the rotation.
    svg(svg::Handle::from_memory(source(palette).into_bytes()))
        .width(size)
        .height(size)
        .rotation(Rotation::Floating(Radians(angle)))
        .into()
}

/// One full turn per second since `start`.
pub(crate) fn angle(start: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() % 1.0) * TAU
}

fn source(palette: Palette) -> String {
    // A 3/4 arc from the top, clockwise to the left, on a 16 unit circle of radius 6.5.
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.5" fill="none" stroke="{}" stroke-width="2"/><path d="M8 1.5 A6.5 6.5 0 1 1 1.5 8" fill="none" stroke="{}" stroke-width="2" stroke-linecap="round"/></svg>"##,
        hex(palette.border),
        hex(palette.muted),
    )
}

fn hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn angle_turns_once_per_second() {
        let start = Instant::now();
        assert_eq!(angle(start, start), 0.0);
        let quarter = angle(start, start + Duration::from_millis(250));
        assert!((quarter - TAU / 4.0).abs() < 1e-4);
        let wrapped = angle(start, start + Duration::from_millis(1250));
        assert!((wrapped - TAU / 4.0).abs() < 1e-3);
    }

    #[test]
    fn source_uses_palette_colors_and_is_stable() {
        let dark = source(Palette::DARK);
        assert!(dark.contains(&hex(Palette::DARK.border)));
        assert!(dark.contains(&hex(Palette::DARK.muted)));
        assert_eq!(dark, source(Palette::DARK));
    }
}
