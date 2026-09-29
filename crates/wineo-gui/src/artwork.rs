use std::path::Path;

use anyhow::{Context, Result};
use iced::widget::image as iced_image;
use image::GenericImageView;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Grid,
    Hero,
    Logo,
}

impl Kind {
    fn max_size(self) -> (u32, u32) {
        match self {
            Self::Grid => (360, 540),
            Self::Hero => (840, 440),
            Self::Logo => (720, 128),
        }
    }
}

pub fn decode(path: &Path, kind: Kind) -> Result<iced_image::Handle> {
    let (width, height, pixels) = if path.extension().is_some_and(|ext| ext == "svg") {
        decode_svg(path, kind)?
    } else {
        decode_raster(path, kind)?
    };
    Ok(iced_image::Handle::from_rgba(width, height, pixels))
}

fn decode_raster(path: &Path, kind: Kind) -> Result<(u32, u32, Vec<u8>)> {
    let image =
        image::open(path).with_context(|| format!("failed to decode {}", path.display()))?;
    let (width, height) = image.dimensions();
    let (target_width, target_height, _) = fit_size(f64::from(width), f64::from(height), kind);
    let image = if (target_width, target_height) == (width, height) {
        image
    } else {
        image.resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        )
    }
    .into_rgba8();
    Ok((target_width, target_height, image.into_raw()))
}

fn fit_size(width: f64, height: f64, kind: Kind) -> (u32, u32, f64) {
    let (max_width, max_height) = kind.max_size();
    let (width_ratio, height_ratio) =
        (f64::from(max_width) / width, f64::from(max_height) / height);
    // Grid and hero art is drawn cover-fit, so it has to fill the box in both
    // directions or a wide hero ends up upscaled and blurry in the banner.
    let fit = if kind == Kind::Logo {
        width_ratio.min(height_ratio)
    } else {
        width_ratio.max(height_ratio)
    };
    let scale = fit.min(1.0);
    let width = ((width * scale).round() as u32).max(1);
    let height = ((height * scale).round() as u32).max(1);
    (width, height, scale)
}

fn decode_svg(path: &Path, kind: Kind) -> Result<(u32, u32, Vec<u8>)> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let size = tree.size();
    let (width, height, scale) = fit_size(f64::from(size.width()), f64::from(size.height()), kind);
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).context("SVG dimensions are too large")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
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
            assert!(decode(&path, Kind::Grid).is_ok(), "{extension}");
        }
        let svg = dir.path().join("grid.svg");
        std::fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"3\"><rect width=\"2\" height=\"3\" fill=\"red\"/></svg>",
        )
        .unwrap();
        assert!(decode(&svg, Kind::Grid).is_ok());
    }

    #[test]
    fn caps_each_art_kind_without_upscaling_or_distorting_aspect() {
        for (kind, source, expected) in [
            (Kind::Grid, (2000.0, 1000.0), (1080, 540)),
            (Kind::Grid, (1000.0, 2000.0), (360, 720)),
            (Kind::Hero, (2000.0, 1000.0), (880, 440)),
            (Kind::Hero, (1920.0, 620.0), (1363, 440)),
            (Kind::Hero, (1000.0, 1000.0), (840, 840)),
            (Kind::Logo, (1600.0, 1000.0), (205, 128)),
        ] {
            let (width, height, scale) = fit_size(source.0, source.1, kind);
            assert_eq!((width, height), expected);
            assert!(scale < 1.0);
            assert!((f64::from(width) / f64::from(height) - source.0 / source.1).abs() < 0.01);
        }

        for kind in [Kind::Grid, Kind::Hero, Kind::Logo] {
            let (width, height, scale) = fit_size(80.0, 40.0, kind);
            assert_eq!((width, height), (80, 40));
            assert_eq!(scale, 1.0);
        }
        let (width, height, _) = fit_size(10_000.0, 100.0, Kind::Logo);
        assert_eq!((width, height), (720, 7));
    }

    #[test]
    fn raster_and_svg_decoding_apply_the_requested_caps() {
        let dir = tempfile::tempdir().unwrap();
        let raster_path = dir.path().join("large.png");
        image::RgbImage::from_pixel(2000, 1000, image::Rgb([40, 100, 180]))
            .save(&raster_path)
            .unwrap();
        let raster = decode_raster(&raster_path, Kind::Hero).unwrap();
        assert_eq!((raster.0, raster.1), (880, 440));

        let svg_path = dir.path().join("large.svg");
        std::fs::write(
            &svg_path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2000\" height=\"1000\"><rect width=\"2000\" height=\"1000\" fill=\"red\"/></svg>",
        )
        .unwrap();
        let svg = decode_svg(&svg_path, Kind::Hero).unwrap();
        assert_eq!((svg.0, svg.1), (880, 440));
    }
}
