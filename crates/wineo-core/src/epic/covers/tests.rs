use std::io::{self, Read};

use super::*;
use crate::epic::fake::Setup;
use crate::steamgriddb::fake::FakeApi;

const APP: &str = "Dlc Game";
const POSTER: &str = "https://cdn.example.invalid/tall.png";

fn png() -> Vec<u8> {
    let mut bytes = io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 6, image::Rgba([10, 20, 30, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn images(tall: bool) -> Vec<Image> {
    let mut images = vec![Image {
        kind: "DieselGameBox".to_owned(),
        url: "https://cdn.example.invalid/wide.png".to_owned(),
    }];
    if tall {
        images.push(Image {
            kind: "DieselGameBoxTall".to_owned(),
            url: POSTER.to_owned(),
        });
    }
    images
}

fn cached_files(paths: &Paths) -> Vec<String> {
    fs::read_dir(paths.epic_covers_dir())
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_cover_is_fetched_once_and_named_by_the_apps_hash() {
    let setup = Setup::new();
    let paths = &setup.paths;
    let fake = FakeApi::default();
    fake.bytes(POSTER, &png());
    let token = Token::new();
    let first = cover(paths, &fake, APP, &images(true), &token).unwrap();
    // An app name with a space never names the file itself.
    assert_eq!(
        first,
        paths
            .epic_covers_dir()
            .join(format!("{}.png", crate::epic::game::key(APP)))
    );
    assert_eq!(fs::read(&first).unwrap(), png());
    assert_eq!(
        cover(paths, &fake, APP, &images(true), &token).unwrap(),
        first
    );
    assert_eq!(fake.urls(), [POSTER]);
    assert_eq!(cached(paths, APP), Some(first));
    assert_eq!(cached(paths, "Other"), None);
}

#[test]
fn no_poster_address_or_a_bad_answer_leaves_no_cache_file() {
    let setup = Setup::new();
    let paths = &setup.paths;
    let fake = FakeApi::default();
    assert!(cover(paths, &fake, APP, &images(false), &Token::new()).is_err());
    assert!(fake.urls().is_empty());
    for body in [&b"<html>not an image</html>"[..], b""] {
        let fake = FakeApi::default();
        fake.bytes(POSTER, body);
        assert!(cover(paths, &fake, APP, &images(true), &Token::new()).is_err());
    }
    let canceled = Token::new();
    canceled.cancel();
    let fake = FakeApi::default();
    fake.bytes(POSTER, &png());
    assert!(cover(paths, &fake, APP, &images(true), &canceled).is_err());
    assert!(fake.urls().is_empty());
    assert!(cached_files(paths).is_empty());
}

/// A body that gives out partway through.
struct Breaks(io::Cursor<Vec<u8>>);

impl Read for Breaks {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer)? {
            0 => Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset")),
            count => Ok(count),
        }
    }
}

#[test]
fn a_server_failure_is_an_outage_and_a_missing_image_is_not() {
    let setup = Setup::new();
    let paths = &setup.paths;
    let down = FakeApi::default();
    down.answer(POSTER, 503, || Box::new(io::empty()));
    let error = cover(paths, &down, APP, &images(true), &Token::new()).unwrap_err();
    assert!(matches!(error, CoverError::Unavailable(_)), "{error:?}");
    let broken = FakeApi::default();
    let png = png();
    broken.answer(POSTER, 200, move || {
        Box::new(Breaks(io::Cursor::new(png[..10].to_vec())))
    });
    let error = cover(paths, &broken, APP, &images(true), &Token::new()).unwrap_err();
    assert!(matches!(error, CoverError::Unavailable(_)), "{error:?}");
    let missing = FakeApi::default();
    missing.answer(POSTER, 404, || Box::new(io::empty()));
    let error = cover(paths, &missing, APP, &images(true), &Token::new()).unwrap_err();
    assert!(matches!(error, CoverError::Skipped(_)), "{error:?}");
    assert!(cached_files(paths).is_empty());
}

#[test]
fn discard_removes_only_that_games_cover() {
    let setup = Setup::new();
    let paths = &setup.paths;
    let fake = FakeApi::default();
    fake.bytes(POSTER, &png());
    let kept = cover(paths, &fake, "Other", &images(true), &Token::new()).unwrap();
    let gone = cover(paths, &fake, APP, &images(true), &Token::new()).unwrap();
    discard(paths, APP).unwrap();
    assert!(!gone.exists());
    assert!(kept.exists());
    assert_eq!(cached(paths, APP), None);
}
