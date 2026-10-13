//! GOG's box art for games a user owns, kept in a host-only cache so a
//! library can show posters before wineo has stored any art for a game.
//!
//! The cover endpoint is public, so no login is needed. A cover is fetched at
//! most once: later calls find the file and never ask GOG.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use wineo_gog::Client;

use super::extras::{fetch_image, link};
use crate::art::{Format, Kind};
use crate::cancel::Token;
use crate::download::Transport;
use crate::files::write_atomic;
use crate::paths::Paths;

/// The cached cover for a game, if one was fetched before. Reads only.
pub fn cached(paths: &Paths, gog_id: u64) -> Option<PathBuf> {
    let dir = paths.gog_covers_dir();
    Format::ALL
        .into_iter()
        .filter(|format| Kind::Grid.accepts(*format))
        .map(|format| dir.join(format!("{gog_id}.{}", format.extension())))
        .find(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
}

/// The cached cover for a game, fetching it first when there is none. Only
/// image formats a Grid accepts are kept, and nothing is written for a
/// failed or non-image answer.
pub fn cover(
    paths: &Paths,
    client: &Client,
    transport: &dyn Transport,
    gog_id: u64,
    token: &Token,
) -> Result<PathBuf> {
    if let Some(path) = cached(paths, gog_id) {
        return Ok(path);
    }
    let links = client.game_art(gog_id)?;
    let Some(url) = link(&links, Kind::Grid) else {
        anyhow::bail!("GOG has no cover for game {gog_id}");
    };
    let (bytes, format) = fetch_image(transport, url, Kind::Grid, token)?;
    let path = paths
        .gog_covers_dir()
        .join(format!("{gog_id}.{}", format.extension()));
    write_atomic(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::io;

    use serde_json::json;

    use super::*;
    use crate::gog::test_support::{Scripted, client_at, setup};
    use crate::steam::api::fixture::reply;
    use crate::steamgriddb::fake::FakeApi;

    const GAME: u64 = 1094900565;
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";
    const ICO: &[u8] = b"\0\0\x01\0\x01\0\x10\x10\0\0\x01\0\x20\0\x04\0\0\0\x16\0\0\0abcd";
    const COVER: &str = "https://images.gog-statics.com/box.jpg";

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
        images.bytes(COVER, JPEG);
        let client = client_at(&server.base);
        let token = Token::new();

        let first = cover(&paths, &client, &images, GAME, &token).unwrap();
        assert_eq!(
            first,
            paths.gog_covers_dir().join(format!("{GAME}.jpg")),
            "named by the detected format"
        );
        assert_eq!(fs::read(&first).unwrap(), JPEG);
        let second = cover(&paths, &client, &images, GAME, &token).unwrap();
        assert_eq!(second, first);

        assert_eq!(server.requests().len(), 1);
        assert_eq!(images.urls(), [COVER]);
        assert_eq!(cached_files(&paths), [format!("{GAME}.jpg")]);
    }

    #[test]
    fn a_cached_cover_needs_neither_a_client_nor_the_network() {
        let (_root, paths) = setup();
        assert_eq!(cached(&paths, GAME), None);
        fs::create_dir_all(paths.gog_covers_dir()).unwrap();
        let file = paths.gog_covers_dir().join(format!("{GAME}.png"));
        fs::write(&file, PNG).unwrap();
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
        huge.bytes_with_length(COVER, JPEG, 33 * 1024 * 1024);
        assert!(cover(&paths, &client, &huge, GAME, &Token::new()).is_err());
        assert!(cached_files(&paths).is_empty());
    }

    #[test]
    fn a_canceled_fetch_writes_nothing() {
        let (_root, paths) = setup();
        let server = gog(art_body(true));
        let images = FakeApi::default();
        images.bytes(COVER, JPEG);
        let client = client_at(&server.base);
        let token = Token::new();
        token.cancel();
        assert!(cover(&paths, &client, &images, GAME, &token).is_err());
        assert!(cached_files(&paths).is_empty());
    }
}
