use std::path::Path;

use anyhow::{Context, Result};
use iced::widget::image as iced_image;

pub fn decode(path: &Path) -> Result<iced_image::Handle> {
    let (width, height, pixels) = if path.extension().is_some_and(|ext| ext == "svg") {
        decode_svg(path)?
    } else {
        let image = image::open(path)
            .with_context(|| format!("failed to decode {}", path.display()))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        (width, height, image.into_raw())
    };
    Ok(iced_image::Handle::from_rgba(width, height, pixels))
}

fn decode_svg(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let size = tree.size();
    let scale = (360.0 / size.width()).min(540.0 / size.height());
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).context("SVG dimensions are too large")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.take();
    for rgba in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(rgba[3]);
        for channel in &mut rgba[..3] {
            *channel = (u16::from(*channel) * 255)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
    Ok((width, height, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_all_grid_formats() {
        let dir = tempfile::tempdir().unwrap();
        let raster = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            3,
            image::Rgb([40, 100, 180]),
        ));
        for (extension, format) in [
            ("png", image::ImageFormat::Png),
            ("jpg", image::ImageFormat::Jpeg),
            ("webp", image::ImageFormat::WebP),
        ] {
            let path = dir.path().join(format!("grid.{extension}"));
            raster.save_with_format(&path, format).unwrap();
            assert!(decode(&path).is_ok(), "{extension}");
        }
        let svg = dir.path().join("grid.svg");
        std::fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"3\"><rect width=\"2\" height=\"3\" fill=\"red\"/></svg>",
        )
        .unwrap();
        assert!(decode(&svg).is_ok());
    }
}
