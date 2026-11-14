//! Coverflow's tilted side covers. A cover is warped once, off the UI
//! thread, into a trapezoid whose far edge is shorter, as if turned toward
//! the middle, and the strip draws that image as is: neither renderer can
//! tilt an image per frame. Covers stay square-cornered, the same in both
//! renderers, since tiny-skia ignores an image's corner radius.

use iced::widget::image::Handle;

use crate::art_cache::Tilt;

/// A fully tilted cover's width, as a share of its flat width.
pub(crate) const WIDTH: f32 = 0.6;
/// How far in the far edge's corners sit from the top and bottom, as a
/// share of the height.
pub(crate) const INSET: f32 = 0.1;

/// The trapezoids a tilted cover fills, for the quads that sit behind and
/// over it, stretched to its bounds.
pub(crate) const LEFT_SHAPE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" preserveAspectRatio="none"><path d="M0 10L100 0L100 100L0 90Z" fill="#000"/></svg>"##;
pub(crate) const RIGHT_SHAPE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" preserveAspectRatio="none"><path d="M0 0L100 10L100 90L0 100Z" fill="#000"/></svg>"##;

/// The top edge's distance below a tilted cover's top at `across`, from 0
/// at its left to 1 at its right, as a share of its height.
pub(crate) fn top_inset(tilt: Tilt, across: f32) -> f32 {
    match tilt {
        Tilt::Left => INSET * (1.0 - across),
        Tilt::Right => INSET * across,
        Tilt::None => 0.0,
    }
}

/// `handle` turned toward the middle: `Left` for a cover left of it, whose
/// far edge is its left one, `Right` the other way. Only its 2:3 middle is
/// used, the part a flat cover shows, so the warp fits a cover's bounds.
/// Perspective-correct, so the far half comes out narrower than the near
/// one, with the slanted top and bottom edges antialiased. `None` for art
/// that isn't decoded pixels.
pub(crate) fn warp(handle: &Handle, tilt: Tilt) -> Option<Handle> {
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        return None;
    };
    if tilt == Tilt::None || *width == 0 || *height == 0 {
        return None;
    }
    let (source_width, source_height) = (*width as usize, *height as usize);
    let (sw, sh) = (source_width as f64, source_height as f64);
    // The poster's 2:3 middle, as a flat cover crops it.
    let (crop_width, crop_height) = if sw / sh > 2.0 / 3.0 {
        (sh * 2.0 / 3.0, sh)
    } else {
        (sw, sw * 1.5)
    };
    let (left, top) = ((sw - crop_width) / 2.0, (sh - crop_height) / 2.0);
    let out_width = ((crop_width * f64::from(WIDTH)).round() as u32).max(1);
    let out_height = (crop_height.round() as u32).max(1);
    let (w, h) = (f64::from(out_width), f64::from(out_height));
    let inset = h * f64::from(INSET);
    let corners = match tilt {
        Tilt::Left => [(0.0, inset), (w, 0.0), (w, h), (0.0, h - inset)],
        _ => [(0.0, 0.0), (w, inset), (w, h - inset), (0.0, h)],
    };
    let (right, bottom) = (left + crop_width, top + crop_height);
    let k = homography(
        corners,
        [(left, top), (right, top), (right, bottom), (left, bottom)],
    );
    let mut out = vec![0u8; out_width as usize * out_height as usize * 4];
    let source = |x: usize, y: usize, channel: usize| {
        f64::from(pixels[(y * source_width + x) * 4 + channel])
    };
    for y in 0..out_height as usize {
        let fy = y as f64 + 0.5;
        for x in 0..out_width as usize {
            let fx = x as f64 + 0.5;
            // How much of this pixel the slanted edges leave inside.
            let across = fx / w;
            let top = h * f64::from(top_inset(tilt, across as f32));
            let coverage = ((fy + 0.5 - top).min(h - top - (fy - 0.5))).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let depth = k[6] * fx + k[7] * fy + k[8];
            let u = ((k[0] * fx + k[1] * fy + k[2]) / depth - 0.5).clamp(0.0, sw - 1.0);
            let v = ((k[3] * fx + k[4] * fy + k[5]) / depth - 0.5).clamp(0.0, sh - 1.0);
            let (x0, y0) = (u as usize, v as usize);
            let (x1, y1) = (
                (x0 + 1).min(source_width - 1),
                (y0 + 1).min(source_height - 1),
            );
            let (fu, fv) = (u - x0 as f64, v - y0 as f64);
            let at = (y * out_width as usize + x) * 4;
            for channel in 0..4 {
                let upper = source(x0, y0, channel) * (1.0 - fu) + source(x1, y0, channel) * fu;
                let lower = source(x0, y1, channel) * (1.0 - fu) + source(x1, y1, channel) * fu;
                let mut value = upper * (1.0 - fv) + lower * fv;
                if channel == 3 {
                    value *= coverage;
                }
                out[at + channel] = (value + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(Handle::from_rgba(out_width, out_height, out))
}

/// The projective map taking each of `to` onto the matching `from`, as the
/// 3x3 matrix's first eight entries and a 1, solved by elimination.
fn homography(to: [(f64, f64); 4], from: [(f64, f64); 4]) -> [f64; 9] {
    let mut rows = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = to[i];
        let (u, v) = from[i];
        rows[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        rows[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for column in 0..8 {
        let pivot = (column..8)
            .max_by(|&a, &b| rows[a][column].abs().total_cmp(&rows[b][column].abs()))
            .unwrap_or(column);
        rows.swap(column, pivot);
        for row in 0..8 {
            if row != column {
                let factor = rows[row][column] / rows[column][column];
                for k in column..9 {
                    rows[row][k] -= factor * rows[column][k];
                }
            }
        }
    }
    let mut matrix = [1.0; 9];
    for i in 0..8 {
        matrix[i] = rows[i][8] / rows[i][i];
    }
    matrix
}

#[cfg(test)]
mod tests;
