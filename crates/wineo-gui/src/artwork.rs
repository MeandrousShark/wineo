use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use iced::widget::image as iced_image;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::{ExtendedColorType, GenericImageView, ImageEncoder, ImageReader};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Grid,
    Hero,
    Logo,
}

/// How art is capped when decoded: a name for the cache, the largest size,
/// and whether the cap is the box the art fits inside or the box it fills.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    name: &'static str,
    max_size: (u32, u32),
    contain: bool,
    // Tiles are decoded in bulk on a first open, where a cheaper filter
    // matters more than the last bit of sharpness at this size.
    filter: image::imageops::FilterType,
}

impl From<Kind> for Spec {
    fn from(kind: Kind) -> Self {
        // Grid and hero art is drawn cover-fit, so it has to fill the box in
        // both directions or a wide hero ends up upscaled and blurry in the
        // banner.
        let (name, max_size, contain) = match kind {
            Kind::Grid => ("grid", (320, 480), false),
            Kind::Hero => ("hero", (840, 440), false),
            Kind::Logo => ("logo", (720, 128), true),
        };
        Self {
            name,
            max_size,
            contain,
            filter: image::imageops::FilterType::Lanczos3,
        }
    }
}

/// A poster in an import grid. These tiles are small and every poster of a
/// long list stays decoded, so the cap is the preview size, not the library's.
pub const TILE: Spec = Spec {
    name: "tile",
    max_size: (
        crate::flow::artwork::POSTER_WIDTH,
        crate::flow::artwork::POSTER_HEIGHT,
    ),
    contain: true,
    filter: image::imageops::FilterType::Triangle,
};

type Pixels = (u32, u32, Vec<u8>);

/// The permits for decoding art off the UI thread, shared by the library and
/// the import grids, since a huge source costs far more memory than a small
/// one and two decoders' worth is already enough to keep up.
pub fn decode_slots() -> Arc<tokio::sync::Semaphore> {
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(crate::MAX_DECODES)))
        .clone()
}

/// Decodes art to at most the size its kind is drawn at. Art that has to be
/// shrunk is cached as a thumbnail under `cache`, since decoding a huge source
/// on every start costs far more than reading back the small copy.
pub fn decode(
    path: &Path,
    spec: impl Into<Spec>,
    cache: Option<&Path>,
) -> Result<iced_image::Handle> {
    decode_as(
        path,
        spec,
        cache,
        path.extension().is_some_and(|ext| ext == "svg"),
    )
}

/// `decode` for a file whose kind is already known, since some files have no
/// extension to tell an SVG from a raster, like Heroic's cached images.
pub fn decode_as(
    path: &Path,
    spec: impl Into<Spec>,
    cache: Option<&Path>,
    svg: bool,
) -> Result<iced_image::Handle> {
    let spec = spec.into();
    // Taken before anything reads the source, so a file replaced mid-decode is
    // stored under its old identity and never served for the new one.
    let identity = cache
        .and_then(|_| fs::metadata(path).ok())
        .map(|metadata| identity(&metadata));
    let entry = |size| {
        cache
            .zip(identity.as_deref())
            .map(|(cache, identity)| Entry::new(cache, path, identity, spec, size))
    };
    let (width, height, pixels) = if svg {
        let (tree, width, height, scale) = open_svg(path, spec)?;
        let entry = entry((width, height)).filter(|_| scale < 1.0);
        cached(entry, || render_svg(&tree, width, height, scale))?
    } else {
        let (source_width, source_height) = reader(path)
            .and_then(|reader| reader.into_dimensions().map_err(Into::into))
            .with_context(|| format!("failed to decode {}", path.display()))?;
        let (width, height, _) = fit_size(f64::from(source_width), f64::from(source_height), spec);
        let entry =
            entry((width, height)).filter(|_| (width, height) != (source_width, source_height));
        cached(entry, || decode_raster(path, spec))?
    };
    Ok(iced_image::Handle::from_rgba(width, height, pixels))
}

pub(crate) fn grayscale(handle: &iced_image::Handle) -> Option<iced_image::Handle> {
    let iced_image::Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        return None;
    };
    let mut pixels = pixels.to_vec();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let gray =
            ((u32::from(pixel[0]) * 2126 + u32::from(pixel[1]) * 7152 + u32::from(pixel[2]) * 722)
                / 10_000) as u8;
        pixel[..3].fill(gray);
    }
    Some(iced_image::Handle::from_rgba(*width, *height, pixels))
}

/// Serves `render` from the thumbnail cache when the output is a shrunk copy.
/// Cache trouble never fails a decode, it only costs the speedup.
fn cached(entry: Option<Entry>, render: impl FnOnce() -> Result<Pixels>) -> Result<Pixels> {
    let Some(entry) = entry else {
        return render();
    };
    if let Some(pixels) = entry.read() {
        return Ok(pixels);
    }
    let pixels = render()?;
    entry.write(&pixels);
    Ok(pixels)
}

/// One cached thumbnail of one source file for one kind at one size.
struct Entry {
    dir: PathBuf,
    file: PathBuf,
    size: (u32, u32),
    source: PathBuf,
}

impl Entry {
    // The spec is in the name because two can round to the same size with
    // different scales, which renders an SVG differently.
    fn new(cache: &Path, source: &Path, identity: &str, spec: Spec, size: (u32, u32)) -> Self {
        let dir = cache.join(hex(&Sha256::digest(source.as_os_str().as_bytes())));
        let file = dir.join(format!(
            "{identity}-{}-{}x{}.png",
            spec.name, size.0, size.1
        ));
        Self {
            dir,
            file,
            size,
            source: source.to_owned(),
        }
    }

    fn read(&self) -> Option<Pixels> {
        let image = image::open(&self.file).ok()?;
        (image.dimensions() == self.size).then(|| {
            let (width, height) = self.size;
            (width, height, image.into_rgba8().into_raw())
        })
    }

    fn write(&self, (width, height, pixels): &Pixels) {
        let _ = self.try_write(*width, *height, pixels);
    }

    fn try_write(&self, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .tempfile_in(&self.dir)?;
        let mut out = BufWriter::new(temp.as_file_mut());
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive)
            .write_image(pixels, width, height, ExtendedColorType::Rgba8)?;
        out.flush()?;
        drop(out);
        temp.persist(&self.file)?;
        // Pruning reads this to know which source the directory belongs to.
        let marker = self.dir.join(SOURCE_FILE);
        if fs::read(&marker).ok().as_deref() != Some(self.source.as_os_str().as_bytes()) {
            let mut temp = tempfile::Builder::new()
                .prefix(TEMP_PREFIX)
                .tempfile_in(&self.dir)?;
            temp.write_all(self.source.as_os_str().as_bytes())?;
            temp.persist(marker)?;
        }
        Ok(())
    }
}

const SOURCE_FILE: &str = "source";
// Leftover temp files are skipped by `prune`, since one may be mid-write in
// another instance.
const TEMP_PREFIX: &str = ".tmp";

fn identity(metadata: &fs::Metadata) -> String {
    let nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos());
    let mut hash = Sha256::new();
    hash.update(nanos.to_le_bytes());
    hash.update(metadata.len().to_le_bytes());
    hex(&hash.finalize()[..8])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Removes cache entries whose source is gone or has changed. Best effort, and
/// independent of any library, so instances that share the host cache never
/// delete each other's live thumbnails.
pub fn prune(cache: &Path) {
    let Ok(dirs) = fs::read_dir(cache) else {
        return;
    };
    for dir in dirs
        .flatten()
        .map(|dir| dir.path())
        .filter(|dir| dir.is_dir())
    {
        prune_dir(&dir);
    }
}

fn prune_dir(dir: &Path) {
    let source = fs::read(dir.join(SOURCE_FILE))
        .ok()
        .map(|bytes| PathBuf::from(OsStr::from_bytes(&bytes)));
    let metadata = source.as_deref().map(fs::metadata);
    let current = match metadata {
        Some(Ok(metadata)) => identity(&metadata),
        // Anything else might be a drive that is briefly unavailable.
        Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => return,
        _ => {
            let _ = fs::remove_dir_all(dir);
            return;
        }
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let keep = format!("{current}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == SOURCE_FILE || name.starts_with(TEMP_PREFIX) || name.starts_with(&keep) {
            continue;
        }
        let _ = fs::remove_file(entry.path());
    }
}

/// Opens a raster by its content before its extension, since a file's name
/// can be missing or wrong.
fn reader(path: &Path) -> Result<ImageReader<io::BufReader<fs::File>>> {
    Ok(ImageReader::open(path)?.with_guessed_format()?)
}

fn decode_raster(path: &Path, spec: impl Into<Spec>) -> Result<Pixels> {
    let spec = spec.into();
    let image = reader(path)
        .and_then(|reader| reader.decode().map_err(Into::into))
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let (width, height) = image.dimensions();
    let (target_width, target_height, _) = fit_size(f64::from(width), f64::from(height), spec);
    let image = if (target_width, target_height) == (width, height) {
        image
    } else {
        image.resize_exact(target_width, target_height, spec.filter)
    }
    .into_rgba8();
    Ok((target_width, target_height, image.into_raw()))
}

fn fit_size(width: f64, height: f64, spec: impl Into<Spec>) -> (u32, u32, f64) {
    let spec = spec.into();
    let (max_width, max_height) = spec.max_size;
    let (width_ratio, height_ratio) =
        (f64::from(max_width) / width, f64::from(max_height) / height);
    let fit = if spec.contain {
        width_ratio.min(height_ratio)
    } else {
        width_ratio.max(height_ratio)
    };
    let scale = fit.min(1.0);
    let width = ((width * scale).round() as u32).max(1);
    let height = ((height * scale).round() as u32).max(1);
    (width, height, scale)
}

fn open_svg(path: &Path, spec: impl Into<Spec>) -> Result<(resvg::usvg::Tree, u32, u32, f64)> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let size = tree.size();
    let (width, height, scale) = fit_size(f64::from(size.width()), f64::from(size.height()), spec);
    Ok((tree, width, height, scale))
}

fn render_svg(tree: &resvg::usvg::Tree, width: u32, height: u32, scale: f64) -> Result<Pixels> {
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).context("SVG dimensions are too large")?;
    resvg::render(
        tree,
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
            assert!(decode(&path, Kind::Grid, None).is_ok(), "{extension}");
        }
        let svg = dir.path().join("grid.svg");
        std::fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"3\"><rect width=\"2\" height=\"3\" fill=\"red\"/></svg>",
        )
        .unwrap();
        assert!(decode(&svg, Kind::Grid, None).is_ok());
    }

    #[test]
    fn caps_each_art_kind_without_upscaling_or_distorting_aspect() {
        for (kind, source, expected) in [
            (Kind::Grid, (2000.0, 1000.0), (960, 480)),
            (Kind::Grid, (1000.0, 2000.0), (320, 640)),
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
        let (tree, width, height, scale) = open_svg(&svg_path, Kind::Hero).unwrap();
        let svg = render_svg(&tree, width, height, scale).unwrap();
        assert_eq!((svg.0, svg.1), (880, 440));
    }

    fn save_png(path: &Path, width: u32, height: u32, color: [u8; 3]) {
        image::RgbImage::from_pixel(width, height, image::Rgb(color))
            .save(path)
            .unwrap();
    }

    fn entries(cache: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for dir in fs::read_dir(cache).into_iter().flatten().flatten() {
            for file in fs::read_dir(dir.path()).unwrap().flatten() {
                if file.file_name() != SOURCE_FILE {
                    found.push(file.path());
                }
            }
        }
        found.sort();
        found
    }

    fn pixel(handle: &iced_image::Handle) -> (u32, u32, Vec<u8>) {
        match handle {
            iced_image::Handle::Rgba {
                width,
                height,
                pixels,
                ..
            } => (*width, *height, pixels.to_vec()),
            other => panic!("unexpected handle {other:?}"),
        }
    }

    #[test]
    fn grayscale_keeps_dimensions_alpha_and_the_original_pixels() {
        let original = iced_image::Handle::from_rgba(1, 1, vec![20, 100, 180, 123]);
        let converted = grayscale(&original).unwrap();
        let (width, height, pixels) = pixel(&converted);
        assert_eq!((width, height), (1, 1));
        assert_eq!(pixels, [88, 88, 88, 123]);
        assert_eq!(pixel(&original).2, [20, 100, 180, 123]);
    }

    #[test]
    fn shrunk_art_is_cached_and_served_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let source = dir.path().join("hero.png");
        save_png(&source, 2000, 1000, [40, 100, 180]);

        let first = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((first.0, first.1), (880, 440));
        let found = entries(&cache);
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-880x440.png")
        );

        // A marker colour only the cached copy has proves the second decode hit.
        save_png(&found[0], 880, 440, [200, 10, 10]);
        let second = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((second.0, second.1), (880, 440));
        assert_eq!(&second.2[..4], &[200, 10, 10, 255]);
        assert_ne!(first.2, second.2);
        assert_eq!(entries(&cache), found);
    }

    #[test]
    fn art_that_needs_no_resize_skips_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let source = dir.path().join("small.png");
        save_png(&source, 80, 40, [1, 2, 3]);
        let handle = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((handle.0, handle.1), (80, 40));
        assert!(!cache.exists());
    }

    #[test]
    fn a_bad_cache_file_falls_back_to_the_original_and_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let source = dir.path().join("hero.png");
        save_png(&source, 2000, 1000, [40, 100, 180]);
        decode(&source, Kind::Hero, Some(&cache)).unwrap();
        let file = entries(&cache).remove(0);

        fs::write(&file, b"not a png").unwrap();
        let handle = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((handle.0, handle.1), (880, 440));
        assert_eq!(&handle.2[..4], &[40, 100, 180, 255]);
        assert_eq!(image::open(&file).unwrap().dimensions(), (880, 440));

        save_png(&file, 100, 100, [0, 0, 0]);
        decode(&source, Kind::Hero, Some(&cache)).unwrap();
        assert_eq!(image::open(&file).unwrap().dimensions(), (880, 440));
    }

    #[test]
    fn a_changed_source_gets_a_new_entry_and_prune_drops_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let source = dir.path().join("hero.png");
        save_png(&source, 2000, 1000, [40, 100, 180]);
        decode(&source, Kind::Hero, Some(&cache)).unwrap();
        let old = entries(&cache);

        save_png(&source, 2400, 1200, [40, 100, 180]);
        decode(&source, Kind::Hero, Some(&cache)).unwrap();
        let both = entries(&cache);
        assert_eq!(both.len(), 2);

        prune(&cache);
        let kept = entries(&cache);
        assert_eq!(kept.len(), 1);
        assert_ne!(kept, old);
        let current = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((current.0, current.1), (880, 440));
        assert_eq!(entries(&cache), kept);
    }

    #[test]
    fn prune_removes_dirs_for_missing_sources_and_missing_markers() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let gone = dir.path().join("gone.png");
        let live = dir.path().join("live.png");
        save_png(&gone, 2000, 1000, [1, 1, 1]);
        save_png(&live, 2000, 1000, [2, 2, 2]);
        decode(&gone, Kind::Hero, Some(&cache)).unwrap();
        decode(&live, Kind::Hero, Some(&cache)).unwrap();
        let orphan = cache.join("orphan");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("x-1x1.png"), b"x").unwrap();
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 3);

        fs::remove_file(&gone).unwrap();
        prune(&cache);
        assert!(!orphan.exists());
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        assert_eq!(entries(&cache).len(), 1);
        prune(&dir.path().join("missing"));
    }

    #[test]
    fn kinds_that_round_to_the_same_size_get_their_own_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let source = dir.path().join("art.svg");
        // Grid and Hero both fit this to 840x480, with slightly different scales.
        fs::write(
            &source,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1750\" height=\"1000\"><rect width=\"1750\" height=\"1000\" fill=\"red\"/></svg>",
        )
        .unwrap();
        let grid = pixel(&decode(&source, Kind::Grid, Some(&cache)).unwrap());
        let hero = pixel(&decode(&source, Kind::Hero, Some(&cache)).unwrap());
        assert_eq!((grid.0, grid.1), (840, 480));
        assert_eq!((hero.0, hero.1), (840, 480));
        let names: Vec<_> = entries(&cache)
            .iter()
            .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().any(|name| name.ends_with("-grid-840x480.png")));
        assert!(names.iter().any(|name| name.ends_with("-hero-840x480.png")));
    }
}
