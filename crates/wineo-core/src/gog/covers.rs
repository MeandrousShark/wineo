//! GOG's box art for games a user owns, kept in a host-only cache so a
//! library can show posters before wineo has stored any art for a game.
//!
//! The cover endpoint is public, so no login is needed. A cover is fetched at
//! most once: later calls find the file and never ask GOG.

use std::fs;
use std::io;
use std::path::PathBuf;

use wineo_gog::{Class, Client};

use super::extras::{fetch_image, link};
use crate::art::{Format, Kind};
use crate::cancel::Token;
use crate::download::Transport;
use crate::files::write_atomic;
use crate::paths::Paths;
use crate::steamgriddb::{DownloadRead, DownloadStatus};

/// The cached cover for a game, if one was fetched before. Reads only.
pub fn cached(paths: &Paths, gog_id: u64) -> Option<PathBuf> {
    let dir = paths.gog_covers_dir();
    Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
        .map(|format| dir.join(format!("{gog_id}.{}", format.extension())))
        .find(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
}

/// Why a cover couldn't be had.
#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    /// The network or GOG's servers are failing (no connection, a timeout, a
    /// 429 or 5xx), so asking for more covers right now is likely to fail
    /// the same way.
    #[error("{0:#}")]
    Unavailable(anyhow::Error),
    /// Only this game's cover is out of reach: GOG has none, it isn't an
    /// image, or the fetch was canceled.
    #[error("{0:#}")]
    Skipped(anyhow::Error),
}

/// Removes a game's cached cover, so the next load fetches it again. For a
/// file that was saved but can't be decoded. Only the exact `<id>.<ext>`
/// names of the cache are touched, and `gog_id` is a number, so it can't
/// name anything else.
pub fn discard(paths: &Paths, gog_id: u64) -> io::Result<()> {
    let dir = paths.gog_covers_dir();
    for format in Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
    {
        match fs::remove_file(dir.join(format!("{gog_id}.{}", format.extension()))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// The cached cover for a game, fetching it first when there is none. Only
/// image formats a Grid accepts are kept, and nothing is written for a
/// failed or non-image answer.
///
/// `Client::game_art` takes no token, so a cancel is honored before it and
/// again before the image is downloaded, not during the art request.
pub fn cover(
    paths: &Paths,
    client: &Client,
    transport: &dyn Transport,
    gog_id: u64,
    token: &Token,
) -> Result<PathBuf, CoverError> {
    if let Some(path) = cached(paths, gog_id) {
        return Ok(path);
    }
    let canceled = || CoverError::Skipped(anyhow::anyhow!("canceled"));
    if token.is_canceled() {
        return Err(canceled());
    }
    let links = client.game_art(gog_id).map_err(|error| {
        let outage = error.class() == Class::Temporary;
        let error = anyhow::Error::new(error);
        if outage {
            CoverError::Unavailable(error)
        } else {
            CoverError::Skipped(error)
        }
    })?;
    let Some(url) = link(&links, Kind::Grid) else {
        return Err(CoverError::Skipped(anyhow::anyhow!(
            "GOG has no cover for game {gog_id}"
        )));
    };
    if token.is_canceled() {
        return Err(canceled());
    }
    let (bytes, format) = fetch_image(transport, url, Kind::Grid, token).map_err(|error| {
        if outage(&error) {
            CoverError::Unavailable(error)
        } else {
            CoverError::Skipped(error)
        }
    })?;
    let path = paths
        .gog_covers_dir()
        .join(format!("{gog_id}.{}", format.extension()));
    write_atomic(&path, bytes).map_err(|error| CoverError::Skipped(error.into()))?;
    Ok(path)
}

/// Whether an image download failed because the server or network did.
fn outage(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<DownloadStatus>()
        .is_some_and(|status| status.0 == 429 || status.0 >= 500)
        || error.downcast_ref::<DownloadRead>().is_some()
        || matches!(
            error.downcast_ref::<crate::error::Error>(),
            Some(crate::error::Error::Network(_))
        )
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use serde_json::json;

    use super::*;
    use crate::gog::test_support::{Scripted, client_at, setup};
    use crate::steam::api::fixture::reply;
    use crate::steamgriddb::fake::FakeApi;

    const GAME: u64 = 1094900565;
    /// A small image that decodes, since a cover is shown as it was saved.
    fn real_png() -> Vec<u8> {
        let mut bytes = io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 6, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    const ICO: &[u8] = b"\0\0\x01\0\x01\0\x10\x10\0\0\x01\0\x20\0\x04\0\0\0\x16\0\0\0abcd";
    const COVER: &str = "https://images.gog-statics.com/box.png";

    /// GOG's v2 answer with a cover link, or without one.
    fn art_body(cover: bool) -> String {
        let links = if cover {
            json!({ "boxArtImage": { "href": COVER } })
        } else {
            json!({})
        };
        json!({
            "_links": links,
            "_embedded": {"product": {"id": GAME, "title": "RimWorld"}},
        })
        .to_string()
    }

    fn gog(art: String) -> Scripted {
        Scripted::start(move |line| {
            if line.contains("/v2/games/") {
                reply(200, &art)
            } else {
                reply(404, "{}")
            }
        })
    }

    fn cached_files(paths: &Paths) -> Vec<String> {
        fs::read_dir(paths.gog_covers_dir())
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn an_uncached_cover_is_fetched_once_and_then_served_from_the_cache() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        images.bytes(COVER, &real_png());
        let client = client_at(&server.base);
        let token = Token::new();

        let first = cover(&paths, &client, &images, GAME, &token).unwrap();
        assert_eq!(
            first,
            paths.gog_covers_dir().join(format!("{GAME}.png")),
            "named by the detected format"
        );
        assert_eq!(fs::read(&first).unwrap(), real_png());
        let second = cover(&paths, &client, &images, GAME, &token).unwrap();
        assert_eq!(second, first);

        assert_eq!(server.requests().len(), 1);
        assert_eq!(images.urls(), [COVER]);
        assert_eq!(cached_files(&paths), [format!("{GAME}.png")]);
    }

    #[test]
    fn a_cached_cover_needs_neither_a_client_nor_the_network() {
        let (_root, paths) = setup();
        assert_eq!(cached(&paths, GAME), None);
        fs::create_dir_all(paths.gog_covers_dir()).unwrap();
        let file = paths.gog_covers_dir().join(format!("{GAME}.png"));
        fs::write(&file, real_png()).unwrap();
        assert_eq!(cached(&paths, GAME), Some(file.clone()));
        // Nothing listens here, so any request would fail.
        let client = client_at("http://127.0.0.1:9");
        let images = FakeApi::default();
        let found = cover(&paths, &client, &images, GAME, &Token::new()).unwrap();
        assert_eq!(found, file);
        assert!(images.urls().is_empty());
    }

    #[test]
    fn a_game_art_failure_leaves_no_cache_file() {
        let (_root, paths) = setup();
        let server = Scripted::start(|_| reply(500, "{}"));
        let images = FakeApi::default();
        let client = client_at(&server.base);
        assert!(cover(&paths, &client, &images, GAME, &Token::new()).is_err());
        assert!(images.urls().is_empty());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_game_without_a_cover_link_leaves_no_cache_file() {
        let (_root, paths) = setup();
        let server = gog(art_body(false));
        let images = FakeApi::default();
        let client = client_at(&server.base);
        assert!(cover(&paths, &client, &images, GAME, &Token::new()).is_err());
        assert!(images.urls().is_empty());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_body_that_is_not_a_grid_image_leaves_no_cache_file() {
        for body in [&b"<html>not an image</html>"[..], ICO, b""] {
            let (_root, paths) = setup();
            let server = gog(art_body(true));
            let images = FakeApi::default();
            images.bytes(COVER, body);
            let client = client_at(&server.base);
            assert!(cover(&paths, &client, &images, GAME, &Token::new()).is_err());
            assert!(cached_files(&paths).is_empty(), "{body:?}");
            assert_eq!(cached(&paths, GAME), None);
        }
    }

    #[test]
    fn a_failed_download_and_a_cover_over_the_size_cap_leave_no_cache_file() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        images.answer(COVER, 404, || Box::new(io::empty()));
        let client = client_at(&server.base);
        assert!(cover(&paths, &client, &images, GAME, &Token::new()).is_err());
        assert!(cached_files(&paths).is_empty());

        let huge = FakeApi::default();
        huge.bytes_with_length(COVER, &real_png(), 33 * 1024 * 1024);
        assert!(cover(&paths, &client, &huge, GAME, &Token::new()).is_err());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_canceled_fetch_writes_nothing() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        images.bytes(COVER, &real_png());
        let client = client_at(&server.base);
        let token = Token::new();
        token.cancel();
        assert!(cover(&paths, &client, &images, GAME, &token).is_err());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_body_streaming_past_the_size_cap_without_a_length_leaves_no_cache_file() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        images.answer(COVER, 200, || {
            Box::new(io::repeat(0).take(crate::art::MAX_BYTES + 1))
        });
        let client = client_at(&server.base);
        let error = cover(&paths, &client, &images, GAME, &Token::new()).unwrap_err();
        assert!(error.to_string().contains("larger than 32 MiB"), "{error}");
        assert!(cached_files(&paths).is_empty());
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
    fn a_body_that_breaks_partway_is_an_outage_and_leaves_no_cache_file() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        let png = real_png();
        images.answer(COVER, 200, move || {
            Box::new(Breaks(io::Cursor::new(png[..10].to_vec())))
        });
        let client = client_at(&server.base);
        let error = cover(&paths, &client, &images, GAME, &Token::new()).unwrap_err();
        assert!(matches!(error, CoverError::Unavailable(_)), "{error:?}");
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn failing_servers_are_outages_and_missing_or_wrong_things_are_per_game() {
        let (_root, paths) = setup();
        let images = FakeApi::default();
        let token = Token::new();
        // GOG's art answer failing with a 5xx, or a 404.
        for (status, outage) in [(503, true), (500, true), (404, false)] {
            let server = Scripted::start(move |_| reply(status, "{}"));
            let error = cover(&paths, &client_at(&server.base), &images, GAME, &token).unwrap_err();
            assert_eq!(
                matches!(error, CoverError::Unavailable(_)),
                outage,
                "{status}: {error:?}"
            );
        }
        // The image download failing the same ways, and not being an image.
        let server = gog(art_body(true));
        let client = client_at(&server.base);
        for (status, body, outage) in [
            (503, Vec::new(), true),
            (404, Vec::new(), false),
            (200, b"<html></html>".to_vec(), false),
        ] {
            let images = FakeApi::default();
            images.answer(COVER, status, move || {
                Box::new(io::Cursor::new(body.clone()))
            });
            let error = cover(&paths, &client, &images, GAME, &token).unwrap_err();
            assert_eq!(
                matches!(error, CoverError::Unavailable(_)),
                outage,
                "{status}: {error:?}"
            );
        }
        // A transport that can't connect.
        struct Down;
        impl Transport for Down {
            fn get(&self, _: &str) -> anyhow::Result<Box<dyn Read>> {
                anyhow::bail!("unused")
            }
            fn request(
                &self,
                _: &str,
                _: Option<&str>,
            ) -> anyhow::Result<crate::download::Response> {
                Err(crate::error::Error::Network("connection refused".into()).into())
            }
        }
        let error = cover(&paths, &client, &Down, GAME, &token).unwrap_err();
        assert!(matches!(error, CoverError::Unavailable(_)), "{error:?}");
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_cancel_between_the_art_request_and_the_download_fetches_no_image() {
        let (_root, paths) = setup();
        let token = Token::new();
        let canceling = token.clone();
        let art = art_body(true);
        let server = Scripted::start(move |_| {
            // GOG answers, and the user cancels while the answer is in flight.
            canceling.cancel();
            reply(200, &art)
        });
        let images = FakeApi::default();
        images.bytes(COVER, &real_png());
        let error = cover(&paths, &client_at(&server.base), &images, GAME, &token).unwrap_err();
        assert!(matches!(error, CoverError::Skipped(_)), "{error:?}");
        assert!(images.urls().is_empty());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn discard_removes_only_that_games_cached_cover() {
        let (_root, paths) = setup();
        let dir = paths.gog_covers_dir();
        fs::create_dir_all(&dir).unwrap();
        for name in [
            format!("{GAME}.png"),
            format!("{GAME}.jpg"),
            format!("{}.png", GAME + 1),
            format!("x{GAME}.png"),
            "notes.txt".to_owned(),
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        discard(&paths, GAME).unwrap();
        discard(&paths, GAME).unwrap();
        let mut left = cached_files(&paths);
        left.sort();
        assert_eq!(
            left,
            [
                format!("{}.png", GAME + 1),
                "notes.txt".to_owned(),
                format!("x{GAME}.png")
            ]
        );
        assert_eq!(cached(&paths, GAME), None);
    }
}
