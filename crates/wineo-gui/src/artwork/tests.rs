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

#[test]
fn decoding_bytes_matches_decoding_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2000\" height=\"1000\"><rect width=\"1000\" height=\"1000\" fill=\"red\"/></svg>";
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(2000, 1000, image::Rgba([40, 100, 180, 255]))
        .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut small = Vec::new();
    image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut io::Cursor::new(&mut small), image::ImageFormat::Png)
        .unwrap();
    let encode = |format| {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            60,
            90,
            image::Rgba([200, 30, 90, 255]),
        ))
        .write_to(&mut io::Cursor::new(&mut bytes), format)
        .unwrap();
        bytes
    };
    let (jpeg, webp, ico) = (
        encode(image::ImageFormat::Jpeg),
        encode(image::ImageFormat::WebP),
        encode(image::ImageFormat::Ico),
    );
    for (name, bytes) in [
        ("large.png", png.as_slice()),
        ("small.png", small.as_slice()),
        ("art.jpg", jpeg.as_slice()),
        ("art.webp", webp.as_slice()),
        ("art.ico", ico.as_slice()),
        ("art.svg", svg.as_bytes()),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        for kind in [Kind::Grid, Kind::Hero, Kind::Logo] {
            assert_eq!(
                pixel(&decode_bytes(bytes, kind).unwrap()),
                pixel(&decode(&path, kind, None).unwrap()),
                "{name} {kind:?}"
            );
        }
    }
    assert!(decode_bytes(b"not an image", Kind::Grid).is_err());
    assert!(decode_bytes(b"<svg", Kind::Grid).is_err());
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
fn a_cached_copy_is_only_served_to_the_filter_that_made_it() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let source = dir.path().join("poster.png");
    save_png(&source, 600, 900, [40, 100, 180]);
    let sharp = Spec {
        filter: image::imageops::FilterType::Lanczos3,
        ..TILE
    };

    decode(&source, sharp, Some(&cache)).unwrap();
    let found = entries(&cache);
    assert_eq!(found.len(), 1);
    save_png(&found[0], 152, 228, [200, 10, 10]);

    let same = pixel(&decode(&source, sharp, Some(&cache)).unwrap());
    assert_eq!(&same.2[..4], &[200, 10, 10, 255]);
    let other = pixel(&decode(&source, TILE, Some(&cache)).unwrap());
    assert_ne!(&other.2[..4], &[200, 10, 10, 255]);
    assert_eq!(entries(&cache).len(), 2);
}

#[test]
fn only_the_import_spec_trusts_content_over_the_extension() {
    let dir = tempfile::tempdir().unwrap();
    // JPEG bytes under the name core would only accept for a PNG.
    let source = dir.path().join("grid.png");
    image::RgbImage::from_pixel(8, 12, image::Rgb([40, 100, 180]))
        .save_with_format(&source, image::ImageFormat::Jpeg)
        .unwrap();
    assert!(decode(&source, Kind::Grid, None).is_err());
    assert!(decode(&source, TILE, None).is_ok());
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
