//! Turns a stored art icon into a themed file under an icon theme.
//!
//! SVG is copied as is. PNG, JPEG, and ICO are decoded, padded to a square
//! with transparency when they aren't one already, downscaled with Lanczos3
//! to the largest standard hicolor size that fits, and written as one PNG.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, ImageReader, Limits, Rgba, RgbaImage};

use crate::art;
use crate::error::{Error, IoContext, Result};
use crate::files::write_atomic;

/// The standard hicolor sizes wineo writes to, largest first. Capped at 256
/// because some launchers, such as Noctalia, never look in `512x512`.
const SIZES: [u32; 12] = [256, 192, 128, 96, 72, 64, 48, 36, 32, 24, 22, 16];

/// The width and height decoding refuses beyond, so a small file that claims
/// a huge image can't exhaust memory.
const MAX_DIMENSION: u32 = 8192;

/// Installs `source`, a stored icon in `format`, under `icons_dir` as `name`.
/// Returns the file it wrote.
pub fn install(
    source: &Path,
    format: art::Format,
    icons_dir: &Path,
    name: &str,
) -> Result<PathBuf> {
    let image_format = match format {
        art::Format::Svg => return install_svg(source, icons_dir, name),
        art::Format::Png => ImageFormat::Png,
        art::Format::Jpeg => ImageFormat::Jpeg,
        art::Format::Ico => ImageFormat::Ico,
        // Not an icon format, but this takes whatever it is given rather
        // than trusting the caller, so it still fails as a decode error.
        art::Format::Webp => ImageFormat::WebP,
    };
    let bytes = fs::read(source).context(|| format!("failed to read {}", source.display()))?;
    let decoded = decode(&bytes, image_format, source)?;
    let squared = square(decoded.to_rgba8());
    let side = squared.width();
    let target = pick_size(side);
    let resized = if target == side {
        squared
    } else {
        imageops::resize(&squared, target, target, FilterType::Lanczos3)
    };
    let mut png = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .expect("encoding a decoded image as PNG cannot fail");
    let path = icons_dir
        .join("hicolor")
        .join(format!("{target}x{target}"))
        .join("apps")
        .join(format!("{name}.png"));
    write_atomic(&path, png)?;
    Ok(path)
}

fn install_svg(source: &Path, icons_dir: &Path, name: &str) -> Result<PathBuf> {
    let bytes = fs::read(source).context(|| format!("failed to read {}", source.display()))?;
    let path = icons_dir
        .join("hicolor")
        .join("scalable")
        .join("apps")
        .join(format!("{name}.svg"));
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// Decodes `bytes` as `format`, refusing anything bigger than
/// `MAX_DIMENSION` on a side before it is ever allocated.
fn decode(bytes: &[u8], format: ImageFormat, source: &Path) -> Result<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(u64::from(MAX_DIMENSION) * u64::from(MAX_DIMENSION) * 4);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader.decode().map_err(|err| Error::IconDecode {
        path: source.to_owned(),
        source: Box::new(err),
    })
}

/// Pads a non-square image to a square with transparent pixels, centered and
/// never cropped. A square image is returned unchanged.
fn square(image: RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width == height {
        return image;
    }
    let side = width.max(height);
    let mut canvas = RgbaImage::from_pixel(side, side, Rgba([0, 0, 0, 0]));
    let x = i64::from((side - width) / 2);
    let y = i64::from((side - height) / 2);
    imageops::overlay(&mut canvas, &image, x, y);
    canvas
}

/// The largest standard hicolor size that fits inside a `side`-pixel square,
/// without upscaling, except a side under 16, which goes to 16.
fn pick_size(side: u32) -> u32 {
    SIZES.into_iter().find(|&size| size <= side).unwrap_or(16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::{ExtendedColorType, GenericImageView, Rgb, RgbImage};

    fn write_png(path: &Path, width: u32, height: u32) {
        let image = RgbaImage::from_pixel(width, height, Rgba([12, 34, 56, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn write_jpeg(path: &Path, width: u32, height: u32) {
        let image = RgbImage::from_pixel(width, height, Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn ico_frame(size: u32) -> IcoFrame<'static> {
        let image = RgbaImage::from_pixel(size, size, Rgba([1, 2, 3, 255]));
        IcoFrame::as_png(image.as_raw(), size, size, ExtendedColorType::Rgba8).unwrap()
    }

    fn read_png(path: &Path) -> DynamicImage {
        image::ImageReader::open(path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
    }

    fn decode_error(err: Error) -> PathBuf {
        match err {
            Error::IconDecode { path, .. } => path,
            other => panic!("expected IconDecode, got {other:?}"),
        }
    }

    #[test]
    fn pick_size_picks_the_largest_that_fits() {
        let cases = [
            (1024, 256),
            (512, 256),
            (300, 256),
            (256, 256),
            (200, 192),
            (128, 128),
            (72, 72),
            (64, 64),
            (40, 36),
            (36, 36),
            (33, 32),
            (23, 22),
            (22, 22),
            (17, 16),
            (16, 16),
            (10, 16),
            (1, 16),
            (0, 16),
        ];
        for (side, expected) in cases {
            assert_eq!(pick_size(side), expected, "side {side}");
        }
    }

    #[test]
    fn a_1024_png_downscales_to_256() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        write_png(&source, 1024, 1024);
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Png, &icons, "wineo-hades-hades").unwrap();
        assert_eq!(
            path,
            icons.join("hicolor/256x256/apps/wineo-hades-hades.png")
        );
        assert_eq!(read_png(&path).dimensions(), (256, 256));
    }

    #[test]
    fn a_300_png_downscales_to_256() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        write_png(&source, 300, 300);
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Png, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/256x256/apps/app.png"));
        assert_eq!(read_png(&path).dimensions(), (256, 256));
    }

    #[test]
    fn a_640x480_jpeg_is_padded_before_it_downscales() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.jpg");
        write_jpeg(&source, 640, 480);
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Jpeg, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/256x256/apps/app.png"));
        let image = read_png(&path).to_rgba8();
        assert_eq!(image.dimensions(), (256, 256));
        // The 640x480 source was padded to 640x640, so the top and bottom
        // bands of the downscaled square are still fully transparent.
        assert_eq!(image.get_pixel(128, 0).0[3], 0);
        assert_eq!(image.get_pixel(128, 255).0[3], 0);
        // The middle is well inside the original image, so it stays opaque.
        assert_eq!(image.get_pixel(128, 128).0[3], 255);
    }

    #[test]
    fn a_40_png_downscales_to_36() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        write_png(&source, 40, 40);
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Png, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/36x36/apps/app.png"));
        assert_eq!(read_png(&path).dimensions(), (36, 36));
    }

    #[test]
    fn a_10_png_upscales_to_16() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        write_png(&source, 10, 10);
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Png, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/16x16/apps/app.png"));
        assert_eq!(read_png(&path).dimensions(), (16, 16));
    }

    #[test]
    fn an_ico_uses_its_largest_image() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.ico");
        let mut bytes = Vec::new();
        IcoEncoder::new(&mut bytes)
            .encode_images(&[ico_frame(16), ico_frame(64), ico_frame(32)])
            .unwrap();
        fs::write(&source, bytes).unwrap();
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Ico, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/64x64/apps/app.png"));
        assert_eq!(read_png(&path).dimensions(), (64, 64));
    }

    #[test]
    fn an_svg_is_copied_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.svg");
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        fs::write(&source, svg).unwrap();
        let icons = root.path().join("icons");
        let path = install(&source, art::Format::Svg, &icons, "app").unwrap();
        assert_eq!(path, icons.join("hicolor/scalable/apps/app.svg"));
        assert_eq!(fs::read(&path).unwrap(), svg);
    }

    #[test]
    fn garbage_bytes_fail_to_decode() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        fs::write(&source, b"not an image").unwrap();
        let icons = root.path().join("icons");
        let err = install(&source, art::Format::Png, &icons, "app").unwrap_err();
        assert_eq!(decode_error(err), source);
    }

    #[test]
    fn webp_is_refused_as_an_icon_format() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.webp");
        fs::write(&source, b"RIFF\0\0\0\0WEBPVP8 ").unwrap();
        let icons = root.path().join("icons");
        let err = install(&source, art::Format::Webp, &icons, "app").unwrap_err();
        assert_eq!(decode_error(err), source);
    }

    /// A PNG with a correct IHDR claiming an image bigger than the limits
    /// allow, and nothing after it. Decoding fails at the header, so this
    /// never has to hold the huge image it claims to be.
    fn huge_png() -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &byte in bytes {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }

        let mut ihdr = Vec::new();
        ihdr.extend(*b"IHDR");
        ihdr.extend(20_000u32.to_be_bytes()); // width
        ihdr.extend(20_000u32.to_be_bytes()); // height
        ihdr.extend([8, 6, 0, 0, 0]); // 8 bit depth, RGBA, no interlace

        let mut png = Vec::new();
        png.extend(*b"\x89PNG\r\n\x1a\n");
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc32(&ihdr).to_be_bytes());
        png
    }

    #[test]
    fn a_huge_advertised_size_is_refused_before_it_is_allocated() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("icon.png");
        fs::write(&source, huge_png()).unwrap();
        let icons = root.path().join("icons");
        let err = install(&source, art::Format::Png, &icons, "app").unwrap_err();
        assert_eq!(decode_error(err), source);
    }
}
