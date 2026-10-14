use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
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
    /// Trusts a raster's content over its extension. The library keeps the
    /// extension, since core accepts formats per kind by file name.
    sniff: bool,
}

impl Spec {
    /// What a cached copy is filed under. The library's Lanczos3 keeps the bare
    /// name so its existing copies stay valid; any other filter is named, since
    /// a copy resized another way is a different picture.
    fn label(self) -> String {
        use image::imageops::FilterType;
        let filter = match self.filter {
            FilterType::Lanczos3 => return self.name.to_owned(),
            FilterType::Nearest => "nearest",
            FilterType::Triangle => "triangle",
            FilterType::CatmullRom => "catmullrom",
            FilterType::Gaussian => "gaussian",
        };
        format!("{}-{filter}", self.name)
    }
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
            sniff: false,
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
    sniff: true,
};

type Pixels = (u32, u32, Vec<u8>);

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
        let (source_width, source_height) = reader(path, spec)
            .and_then(|reader| reader.into_dimensions().map_err(Into::into))
            .with_context(|| format!("failed to decode {}", path.display()))?;
        let (width, height, _) = fit_size(f64::from(source_width), f64::from(source_height), spec);
        let entry =
            entry((width, height)).filter(|_| (width, height) != (source_width, source_height));
        cached(entry, || decode_raster(path, spec))?
    };
    Ok(iced_image::Handle::from_rgba(width, height, pixels))
}

/// Decodes art held in memory, to the same sizes and with the same SVG handling
/// as `decode`. Nothing is cached, since there is no source file to key on.
pub fn decode_bytes(bytes: &[u8], kind: Kind) -> Result<iced_image::Handle> {
    let (width, height, pixels) =
        if wineo_core::art::detect(bytes) == Some(wineo_core::art::Format::Svg) {
            let (tree, width, height, scale) = parse_svg(bytes, kind, "the SVG")?;
            render_svg(&tree, width, height, scale)?
        } else {
            let image = image::load_from_memory(bytes).context("failed to decode the image")?;
            fit_raster(image, kind)
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
            spec.label(),
            size.0,
            size.1
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

/// Opens a raster by its extension, or by its content first for a spec that
/// sniffs, since a cached file's name can be missing or wrong.
fn reader(path: &Path, spec: Spec) -> Result<ImageReader<io::BufReader<fs::File>>> {
    let reader = ImageReader::open(path)?;
    Ok(if spec.sniff {
        reader.with_guessed_format()?
    } else {
        reader
    })
}

fn decode_raster(path: &Path, spec: impl Into<Spec>) -> Result<Pixels> {
    let spec = spec.into();
    let image = reader(path, spec)
        .and_then(|reader| reader.decode().map_err(Into::into))
        .with_context(|| format!("failed to decode {}", path.display()))?;
    Ok(fit_raster(image, spec))
}

fn fit_raster(image: image::DynamicImage, spec: impl Into<Spec>) -> Pixels {
    let spec = spec.into();
    let (width, height) = image.dimensions();
    let (target_width, target_height, _) = fit_size(f64::from(width), f64::from(height), spec);
    let image = if (target_width, target_height) == (width, height) {
        image
    } else {
        image.resize_exact(target_width, target_height, spec.filter)
    }
    .into_rgba8();
    (target_width, target_height, image.into_raw())
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
    parse_svg(&bytes, spec, &path.display().to_string())
}

fn parse_svg(
    bytes: &[u8],
    spec: impl Into<Spec>,
    name: &str,
) -> Result<(resvg::usvg::Tree, u32, u32, f64)> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
        .with_context(|| format!("failed to parse {name}"))?;
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
mod tests;
