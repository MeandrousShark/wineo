//! Fetching an environment's art from SteamGridDB, one kind at a time.

use anyhow::{Result, anyhow, bail};

use crate::art::{self, Downloaded, Kind, Source, Stored, detect, find, spot_dir};
use crate::cancel::Token;
use crate::environment::EnvironmentIdentity;
use crate::error::Error;
use crate::paths::Paths;
use crate::steamgriddb::{Api, Filters, Image, rank};

/// Which kinds a fetch may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only kinds without art.
    Fill,
    /// Every kind, manual and unrecorded art included.
    Replace,
    /// This one asset, replacing whatever the kind has.
    Asset(u64),
}

/// How fetching one kind went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Stored {
        asset: u64,
        stored: Stored,
    },
    /// The kind already has art. None is unrecorded art.
    Skipped {
        source: Option<Source>,
    },
    /// Nothing usable was left after the filters.
    NoneFound,
    /// `Mode::Asset` named an image the game's filtered list doesn't have.
    AssetNotFound,
}

/// Fetches `kind` for `target`, or the environment for None, from `game`.
/// Candidates are tried best first, and one whose bytes are not a format the
/// kind takes is passed over for the next.
#[allow(clippy::too_many_arguments)]
pub fn fetch_kind(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    game: u64,
    language: &str,
    mode: Mode,
) -> Result<Fetched> {
    fetch_kind_for(paths, env, target, kind, api, game, language, mode, None)
}

/// Like `fetch_kind`, except that nothing is written unless the environment
/// is still the one `expected_identity` was read from. The check is made
/// under the art lock, after the download.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_kind_for(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    game: u64,
    language: &str,
    mode: Mode,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Fetched> {
    art::check_spot(paths, env, target)?;
    if mode == Mode::Fill
        && let Some(skipped) = existing(paths, env, target, kind)?
    {
        return Ok(skipped);
    }
    let image = match choose_for(api, game, kind, language, mode)? {
        Choice::Image(image) => image,
        Choice::NoneFound => return Ok(Fetched::NoneFound),
        Choice::AssetNotFound => return Ok(Fetched::AssetNotFound),
    };
    publish_with_cancel(
        paths,
        env,
        target,
        kind,
        &image,
        mode,
        None,
        expected_identity,
    )
}

/// Fetches SteamGridDB art for missing kinds, using Steam's app ID to find the
/// corresponding SteamGridDB game. A per-kind failure is returned alongside
/// that kind so later kinds can still be tried.
pub fn fetch_steam(
    paths: &Paths,
    env: &str,
    app_id: u64,
    api: &Api<'_>,
    language: &str,
    kinds: &[Kind],
) -> Result<Vec<(Kind, Result<Fetched>)>> {
    let mut missing = Vec::new();
    for &kind in kinds {
        if existing(paths, env, None, kind)?.is_none() {
            missing.push(kind);
        }
    }
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let game = api
        .steam_game(app_id)?
        .ok_or_else(|| anyhow!("SteamGridDB has no game for Steam app {app_id}"))?;
    art::save_game(paths, env, game.id)?;

    Ok(missing
        .into_iter()
        .map(|kind| {
            (
                kind,
                fetch_kind(paths, env, None, kind, api, game.id, language, Mode::Fill),
            )
        })
        .collect())
}

/// Chooses and downloads the best usable ranked image without an environment.
pub fn choose(api: &Api, game: u64, kind: Kind, language: &str) -> Result<Option<Downloaded>> {
    match choose_for(api, game, kind, language, Mode::Fill)? {
        Choice::Image(image) => Ok(Some(image)),
        Choice::NoneFound | Choice::AssetNotFound => Ok(None),
    }
}

/// Keeps existing art intact until the selected image is downloaded and validated.
#[allow(clippy::too_many_arguments)]
pub fn use_image(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    api: &Api,
    image: &Image,
    progress: impl FnMut(u64, Option<u64>),
    cancel: &Token,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Fetched> {
    art::check_spot(paths, env, target)?;
    let bytes = api.download_with_progress(&image.url, progress, cancel)?;
    let Some(format) = detect(&bytes).filter(|format| kind.accepts(*format)) else {
        bail!("asset {} is not a {} image", image.id, art::accepted(kind));
    };
    if cancel.is_canceled() {
        return Err(Error::Canceled("download").into());
    }
    publish_with_cancel(
        paths,
        env,
        target,
        kind,
        &Downloaded {
            asset: image.id,
            bytes,
            format,
        },
        Mode::Replace,
        Some(cancel),
        expected_identity,
    )
}

enum Choice {
    Image(Downloaded),
    NoneFound,
    AssetNotFound,
}

fn choose_for(api: &Api, game: u64, kind: Kind, language: &str, mode: Mode) -> Result<Choice> {
    // The user picked an asset by hand, so none of ranking's filters apply.
    let filters = match mode {
        Mode::Asset(_) => Filters::None,
        Mode::Fill | Mode::Replace => Filters::Ranked,
    };
    let mut images = api.images(game, kind, filters)?;
    if let Mode::Asset(asset) = mode {
        images.retain(|image| image.id == asset);
        if images.is_empty() {
            return Ok(Choice::AssetNotFound);
        }
    }
    rank(kind, &mut images, language);
    for image in images {
        let bytes = api.download(&image.url)?;
        let Some(format) = detect(&bytes).filter(|format| kind.accepts(*format)) else {
            if let Mode::Asset(asset) = mode {
                bail!("asset {asset} is not a {} image", art::accepted(kind));
            }
            continue;
        };
        return Ok(Choice::Image(Downloaded {
            asset: image.id,
            bytes,
            format,
        }));
    }
    Ok(Choice::NoneFound)
}

/// Publishes a downloaded image, keeping existing art in Fill mode.
pub fn publish(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    mode: Mode,
) -> Result<Fetched> {
    publish_with_cancel(paths, env, target, kind, image, mode, None, None)
}

#[allow(clippy::too_many_arguments)]
fn publish_with_cancel(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
    image: &Downloaded,
    mode: Mode,
    cancel: Option<&Token>,
    expected_identity: Option<EnvironmentIdentity>,
) -> Result<Fetched> {
    art::check_spot(paths, env, target)?;
    if mode == Mode::Fill
        && let Some(skipped) = existing(paths, env, target, kind)?
    {
        return Ok(skipped);
    }
    let published = match cancel {
        Some(cancel) => art::publish_fetched_with_cancel(
            paths,
            env,
            target,
            kind,
            image,
            mode != Mode::Fill,
            Some(cancel),
            expected_identity,
        )?,
        None => match expected_identity {
            Some(expected_identity) => art::publish_fetched_with_cancel(
                paths,
                env,
                target,
                kind,
                image,
                mode != Mode::Fill,
                None,
                Some(expected_identity),
            )?,
            None => art::publish_fetched(paths, env, target, kind, image, mode != Mode::Fill)?,
        },
    };
    Ok(match published {
        Some(stored) => Fetched::Stored {
            asset: image.asset,
            stored,
        },
        None => existing(paths, env, target, kind)?.unwrap_or(Fetched::Skipped { source: None }),
    })
}

/// The skip for a kind that already has art, if it has any.
pub(crate) fn existing(
    paths: &Paths,
    env: &str,
    target: Option<&str>,
    kind: Kind,
) -> Result<Option<Fetched>> {
    let art_dir = paths.art_dir(env);
    let config = art::ArtConfig::load(&art_dir)?;
    let record = config.record(target, kind);
    let found = find(&spot_dir(&art_dir, target), kind, record.map(|r| r.format))?;
    Ok(found.map(|found| Fetched::Skipped {
        source: record
            .filter(|record| record.format == found.format)
            .map(|record| record.source.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::art::{ArtConfig, Format, Record};
    use crate::steamgriddb::fake::FakeApi;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";

    fn setup() -> (tempfile::TempDir, Paths) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let config = paths.environment_config("hades");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            "prefix = \"~/p\"\nrunner = \"wine\"\narch = \"win64\"\n[targets.server]\nexe = \"s.exe\"\n",
        )
        .unwrap();
        (root, paths)
    }

    /// Serves `images` as the only page for `kind` of game 7, each image's
    /// download answering with its bytes.
    fn serve(api: &FakeApi, kind: Kind, images: &[(u64, &[u8])]) {
        serve_with(api, kind, Filters::Ranked, images);
    }

    fn serve_with(api: &FakeApi, kind: Kind, filters: Filters, images: &[(u64, &[u8])]) {
        let listed: Vec<_> = images
            .iter()
            .map(|(id, _)| json!({ "id": id, "url": format!("https://cdn.example/{id}"), "score": -(*id as i64) }))
            .collect();
        api.json(Api::images_url(7, kind, 0, filters), json!(listed));
        api.json(Api::images_url(7, kind, 1, filters), json!([]));
        for (id, bytes) in images {
            api.bytes(format!("https://cdn.example/{id}"), bytes);
        }
    }

    fn fetch(
        paths: &Paths,
        api: &FakeApi,
        target: Option<&str>,
        kind: Kind,
        mode: Mode,
    ) -> Result<Fetched> {
        fetch_kind(
            paths,
            "hades",
            target,
            kind,
            &Api::new(api, "k"),
            7,
            "en",
            mode,
        )
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn manual(paths: &Paths, name: &str, bytes: &[u8]) -> PathBuf {
        let art = paths.art_dir("hades");
        fs::create_dir_all(&art).unwrap();
        let path = art.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn replace_environment(paths: &Paths) {
        let config = paths.environment_config("hades");
        let replacement = config.with_extension("replacement");
        fs::write(&replacement, fs::read(&config).unwrap()).unwrap();
        fs::rename(replacement, config).unwrap();
    }

    #[test]
    fn fetch_stores_the_best_image_and_records_it() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Grid, &[(3, JPEG), (1, PNG)]);
        let fetched = fetch(&paths, &api, None, Kind::Grid, Mode::Fill).unwrap();
        let art = paths.art_dir("hades");
        let Fetched::Stored { asset, stored } = fetched else {
            panic!("{fetched:?}");
        };
        assert_eq!((asset, stored.format), (1, Format::Png));
        assert_eq!(fs::read(art.join("grid.png")).unwrap(), PNG);
        assert_eq!(
            ArtConfig::load(&art).unwrap().record(None, Kind::Grid),
            Some(&Record {
                format: Format::Png,
                source: Source::SteamGridDb(1),
            })
        );
        assert_eq!(names(&art), ["art.toml", "grid.png"]);
        // Only the best candidate was downloaded.
        assert!(!api.urls().contains(&"https://cdn.example/3".to_owned()));
    }

    #[test]
    fn fetch_steam_uses_the_app_id_and_only_requests_missing_kinds() {
        let (_root, paths) = setup();
        manual(&paths, "hero.png", PNG);
        let api = FakeApi::default();
        api.json(
            format!("{}/games/steam/1086940", crate::steamgriddb::API),
            json!({ "id": 77, "name": "Baldur's Gate 3" }),
        );
        api.json(
            Api::images_url(77, Kind::Grid, 0, Filters::Ranked),
            json!([{ "id": 9, "url": "https://cdn.example/9.png" }]),
        );
        api.json(
            Api::images_url(77, Kind::Grid, 1, Filters::Ranked),
            json!([]),
        );
        api.bytes("https://cdn.example/9.png", PNG);

        let results = fetch_steam(
            &paths,
            "hades",
            1086940,
            &Api::new(&api, "k"),
            "en",
            &[Kind::Grid, Kind::Hero],
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            &results[0].1,
            Ok(Fetched::Stored { asset: 9, .. })
        ));
        assert_eq!(art::saved_game(&paths, "hades").unwrap(), Some(77));
        assert_eq!(
            fs::read(paths.art_dir("hades").join("grid.png")).unwrap(),
            PNG
        );
        assert_eq!(
            fs::read(paths.art_dir("hades").join("hero.png")).unwrap(),
            PNG
        );
        assert_eq!(api.urls().len(), 4);
        assert!(!api.urls().iter().any(|url| url.contains("/heroes/")));
    }

    #[test]
    fn fetch_steam_keeps_trying_after_a_kind_fails() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        api.json(
            format!("{}/games/steam/1086940", crate::steamgriddb::API),
            json!({ "id": 77, "name": "Baldur's Gate 3" }),
        );
        api.answer(
            Api::images_url(77, Kind::Icon, 0, Filters::Ranked),
            500,
            || Box::new(io::empty()),
        );
        api.json(
            Api::images_url(77, Kind::Grid, 0, Filters::Ranked),
            json!([]),
        );

        let results = fetch_steam(
            &paths,
            "hades",
            1086940,
            &Api::new(&api, "k"),
            "en",
            &[Kind::Icon, Kind::Grid],
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_err());
        assert!(matches!(&results[1].1, Ok(Fetched::NoneFound)));
        assert!(api.urls().iter().any(|url| url.contains("/grids/")));
    }

    #[test]
    fn choose_downloads_without_an_environment_and_publish_stores_it() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Icon, &[(1, b"bad"), (2, PNG)]);
        let config = paths.environment_config("hades");
        let saved = fs::read(&config).unwrap();
        fs::remove_file(&config).unwrap();
        let image = choose(&Api::new(&api, "k"), 7, Kind::Icon, "en")
            .unwrap()
            .unwrap();
        assert_eq!(image.asset, 2);
        assert_eq!(image.bytes, PNG);
        assert_eq!(image.format, Format::Png);
        fs::write(config, saved).unwrap();
        let requests = api.urls().len();
        assert!(matches!(
            publish(&paths, "hades", None, Kind::Icon, &image, Mode::Fill).unwrap(),
            Fetched::Stored { asset: 2, .. }
        ));
        assert_eq!(api.urls().len(), requests);
        assert_eq!(
            fs::read(paths.art_dir("hades").join("icon.png")).unwrap(),
            PNG
        );
    }

    #[test]
    fn publish_fill_keeps_art_added_after_choose() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Hero, &[(4, JPEG)]);
        let image = choose(&Api::new(&api, "k"), 7, Kind::Hero, "en")
            .unwrap()
            .unwrap();
        let manual = manual(&paths, "hero.png", PNG);
        assert_eq!(
            publish(&paths, "hades", None, Kind::Hero, &image, Mode::Fill).unwrap(),
            Fetched::Skipped { source: None }
        );
        assert_eq!(fs::read(manual).unwrap(), PNG);
    }

    #[test]
    fn use_image_publishes_the_selected_url_without_listing_again() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let image = Image {
            id: 42,
            url: "https://cdn.example/selected.png".into(),
            ..Image::default()
        };
        api.bytes(&image.url, PNG);
        let mut progress = Vec::new();
        let fetched = use_image(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &Api::new(&api, "k"),
            &image,
            |received, total| progress.push((received, total)),
            &Token::new(),
            None,
        )
        .unwrap();
        assert!(matches!(fetched, Fetched::Stored { asset: 42, .. }));
        assert_eq!(api.urls(), [image.url]);
        assert_eq!(progress, [(PNG.len() as u64, Some(PNG.len() as u64))]);
        assert_eq!(
            fs::read(paths.art_dir("hades").join("grid.png")).unwrap(),
            PNG
        );
    }

    #[test]
    fn use_image_rejects_replacement_after_final_progress() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let image = Image {
            id: 70,
            url: "https://cdn.example/replacement.jpg".into(),
            ..Image::default()
        };
        api.bytes(&image.url, JPEG);
        let identity = EnvironmentIdentity::read(&paths, "hades").unwrap();
        let source = manual(&paths, "source.png", PNG);
        art::set_manual(&paths, "hades", None, Kind::Grid, &source, None).unwrap();
        let art_dir = paths.art_dir("hades");
        let saved_art = fs::read(art_dir.join("grid.png")).unwrap();
        let saved_record = fs::read(art_dir.join(art::ART_FILE)).unwrap();
        let mut completed = false;

        let err = use_image(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &Api::new(&api, "k"),
            &image,
            |received, total| {
                if Some(received) == total {
                    replace_environment(&paths);
                    completed = true;
                }
            },
            &Token::new(),
            Some(identity),
        )
        .unwrap_err();

        assert!(completed);
        assert_ne!(EnvironmentIdentity::read(&paths, "hades"), Some(identity));
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::ArtEnvironmentChanged(name)) if name == "hades"
        ));
        assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), saved_art);
        assert_eq!(fs::read(art_dir.join(art::ART_FILE)).unwrap(), saved_record);
        assert!(!art_dir.join("grid.jpg").exists());
    }

    #[test]
    fn use_image_rechecks_identity_after_waiting_for_art_lock() {
        let (_root, paths) = setup();
        let image = Image {
            id: 71,
            url: "https://cdn.example/locked.jpg".into(),
            ..Image::default()
        };
        let identity = EnvironmentIdentity::read(&paths, "hades").unwrap();
        let source = manual(&paths, "source.png", PNG);
        art::set_manual(&paths, "hades", None, Kind::Grid, &source, None).unwrap();
        let art_dir = paths.art_dir("hades");
        let saved_art = fs::read(art_dir.join("grid.png")).unwrap();
        let saved_record = fs::read(art_dir.join(art::ART_FILE)).unwrap();
        let held = crate::operation::Locks::new(&paths)
            .config(&art_dir.join(art::ART_FILE))
            .unwrap();
        let (progress, seen) = std::sync::mpsc::channel();
        let worker = thread::spawn({
            let paths = paths.clone();
            move || {
                let api = FakeApi::default();
                api.bytes(&image.url, JPEG);
                use_image(
                    &paths,
                    "hades",
                    None,
                    Kind::Grid,
                    &Api::new(&api, "k"),
                    &image,
                    |_, _| progress.send(()).unwrap(),
                    &Token::new(),
                    Some(identity),
                )
            }
        });
        seen.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(30));
        replace_environment(&paths);
        drop(held);

        let err = worker.join().unwrap().unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::ArtEnvironmentChanged(name)) if name == "hades"
        ));
        assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), saved_art);
        assert_eq!(fs::read(art_dir.join(art::ART_FILE)).unwrap(), saved_record);
        assert!(!art_dir.join("grid.jpg").exists());
    }

    #[test]
    fn canceled_use_image_leaves_prior_art_untouched() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let old = manual(&paths, "grid.png", b"prior artwork");
        let image = Image {
            id: 43,
            url: "https://cdn.example/large.png".into(),
            ..Image::default()
        };
        let mut bytes = vec![b'x'; 128 * 1024];
        bytes[..PNG.len()].copy_from_slice(PNG);
        api.bytes(&image.url, &bytes);
        let cancel = Token::new();
        let err = use_image(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &Api::new(&api, "k"),
            &image,
            |_, _| cancel.cancel(),
            &cancel,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Canceled("download"))
        ));
        assert_eq!(fs::read(old).unwrap(), b"prior artwork");
    }

    #[test]
    fn cancellation_while_waiting_for_the_art_lock_preserves_art_and_record() {
        let (_root, paths) = setup();
        let prior = Downloaded {
            asset: 45,
            bytes: PNG.to_vec(),
            format: Format::Png,
        };
        publish(&paths, "hades", None, Kind::Grid, &prior, Mode::Replace).unwrap();
        let art_dir = paths.art_dir("hades");
        let old_art = fs::read(art_dir.join("grid.png")).unwrap();
        let old_config = fs::read(art_dir.join(art::ART_FILE)).unwrap();
        let held = crate::operation::Locks::new(&paths)
            .config(&art_dir.join(art::ART_FILE))
            .unwrap();
        let cancel = Token::new();
        let thread_cancel = cancel.clone();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            thread_cancel.cancel();
            drop(held);
        });

        let replacement = Downloaded {
            asset: 46,
            bytes: JPEG.to_vec(),
            format: Format::Jpeg,
        };
        let err = publish_with_cancel(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &replacement,
            Mode::Replace,
            Some(&cancel),
            None,
        )
        .unwrap_err();
        release.join().unwrap();

        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Canceled("download"))
        ));
        assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), old_art);
        assert_eq!(fs::read(art_dir.join(art::ART_FILE)).unwrap(), old_config);
        assert_eq!(
            ArtConfig::load(&art_dir)
                .unwrap()
                .record(None, Kind::Grid)
                .unwrap()
                .source,
            Source::SteamGridDb(45)
        );
    }

    #[test]
    fn use_image_refuses_an_unstorable_format_with_the_asset_error() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let prior = Downloaded {
            asset: 47,
            bytes: PNG.to_vec(),
            format: Format::Png,
        };
        publish(&paths, "hades", None, Kind::Grid, &prior, Mode::Replace).unwrap();
        let art_dir = paths.art_dir("hades");
        let old_art = fs::read(art_dir.join("grid.png")).unwrap();
        let old_config = fs::read(art_dir.join(art::ART_FILE)).unwrap();
        let image = Image {
            id: 44,
            url: "https://cdn.example/not-an-image".into(),
            ..Image::default()
        };
        api.bytes(&image.url, b"not an image");
        let err = use_image(
            &paths,
            "hades",
            None,
            Kind::Grid,
            &Api::new(&api, "k"),
            &image,
            |_, _| {},
            &Token::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "asset 44 is not a PNG, SVG, JPEG, or WebP image"
        );
        assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), old_art);
        assert_eq!(fs::read(art_dir.join(art::ART_FILE)).unwrap(), old_config);
        assert_eq!(
            ArtConfig::load(&art_dir)
                .unwrap()
                .record(None, Kind::Grid)
                .unwrap()
                .source,
            Source::SteamGridDb(47)
        );
    }

    #[test]
    fn kinds_with_art_are_skipped_without_asking_the_api() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let unrecorded = manual(&paths, "icon.png", b"mine");
        assert_eq!(
            fetch(&paths, &api, None, Kind::Icon, Mode::Fill).unwrap(),
            Fetched::Skipped { source: None }
        );
        assert!(api.urls().is_empty());
        assert_eq!(fs::read(&unrecorded).unwrap(), b"mine");

        let file = paths.art_dir("hades").join("a.png");
        fs::write(&file, PNG).unwrap();
        fs::remove_file(&unrecorded).unwrap();
        crate::art::set_manual(&paths, "hades", None, Kind::Logo, &file, None).unwrap();
        assert_eq!(
            fetch(&paths, &api, None, Kind::Logo, Mode::Fill).unwrap(),
            Fetched::Skipped {
                source: Some(Source::Manual("a.png".into()))
            }
        );
        assert!(api.urls().is_empty());
    }

    #[test]
    fn replace_takes_manual_and_unrecorded_art() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Hero, &[(4, JPEG)]);
        let art = paths.art_dir("hades");
        manual(&paths, "hero.png", b"mine");
        manual(&paths, "hero.webp", b"also mine");
        let fetched = fetch(&paths, &api, None, Kind::Hero, Mode::Replace).unwrap();
        let Fetched::Stored { stored, .. } = fetched else {
            panic!("{fetched:?}");
        };
        assert_eq!(stored.path, art.join("hero.jpg"));
        assert_eq!(
            stored.removed,
            [art.join("hero.png"), art.join("hero.webp")]
        );
        assert_eq!(names(&art), ["art.toml", "hero.jpg"]);

        // Replacing the same format writes over the file in place.
        serve(&api, Kind::Hero, &[(5, JPEG)]);
        fetch(&paths, &api, None, Kind::Hero, Mode::Replace).unwrap();
        assert_eq!(names(&art), ["art.toml", "hero.jpg"]);
        assert_eq!(
            ArtConfig::load(&art)
                .unwrap()
                .record(None, Kind::Hero)
                .unwrap()
                .source,
            Source::SteamGridDb(5)
        );
    }

    #[test]
    fn an_asset_is_looked_up_without_any_filter() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        // A landscape or NSFW grid that only the unfiltered list has.
        serve_with(&api, Kind::Grid, Filters::Ranked, &[(1, PNG)]);
        serve_with(&api, Kind::Grid, Filters::None, &[(1, PNG), (9, JPEG)]);
        manual(&paths, "grid.png", b"mine");
        let fetched = fetch(&paths, &api, None, Kind::Grid, Mode::Asset(9)).unwrap();
        assert!(
            matches!(fetched, Fetched::Stored { asset: 9, .. }),
            "{fetched:?}"
        );
        let art = paths.art_dir("hades");
        assert_eq!(names(&art), ["art.toml", "grid.jpg"]);
        assert_eq!(
            ArtConfig::load(&art)
                .unwrap()
                .record(None, Kind::Grid)
                .unwrap()
                .source,
            Source::SteamGridDb(9)
        );
        let lists: Vec<String> = api
            .urls()
            .into_iter()
            .filter(|url| url.contains("/grids/"))
            .collect();
        assert!(!lists.is_empty());
        for url in &lists {
            assert!(
                url.contains("?types=static,animated&nsfw=any&humor=any&epilepsy=any&page="),
                "{url}"
            );
            assert!(!url.contains("dimensions"), "{url}");
        }
        assert_eq!(
            fetch(&paths, &api, None, Kind::Grid, Mode::Asset(2)).unwrap(),
            Fetched::AssetNotFound
        );
    }

    #[test]
    fn ranked_fetches_keep_every_filter() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Grid, &[(1, PNG)]);
        fetch(&paths, &api, None, Kind::Grid, Mode::Replace).unwrap();
        let lists: Vec<String> = api
            .urls()
            .into_iter()
            .filter(|url| url.contains("/grids/"))
            .collect();
        assert!(!lists.is_empty());
        for url in &lists {
            assert!(
                url.contains(
                    "?types=static&nsfw=false&humor=false&dimensions=600x900,342x482,660x930&page="
                ),
                "{url}"
            );
        }
    }

    #[test]
    fn an_asset_the_kind_cannot_store_is_refused_and_leaves_nothing() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        // A WebM video, which sniffs as nothing art can be.
        let webm: &[u8] = b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81";
        serve_with(&api, Kind::Grid, Filters::None, &[(9, webm)]);
        manual(&paths, "grid.png", b"mine");
        let err = fetch(&paths, &api, None, Kind::Grid, Mode::Asset(9)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "asset 9 is not a PNG, SVG, JPEG, or WebP image"
        );
        let art = paths.art_dir("hades");
        assert_eq!(names(&art), ["grid.png"]);
        assert_eq!(fs::read(art.join("grid.png")).unwrap(), b"mine");

        // An ICO is an image, just not one a grid can be.
        let ico: &[u8] = &[
            0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0, 4, 0, 0, 0, 22, 0, 0, 0, 1, 2, 3, 4,
        ];
        serve_with(&api, Kind::Grid, Filters::None, &[(9, ico)]);
        let err = fetch(&paths, &api, None, Kind::Grid, Mode::Asset(9)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "asset 9 is not a PNG, SVG, JPEG, or WebP image"
        );
        assert_eq!(names(&art), ["grid.png"]);
    }

    #[test]
    fn unusable_candidates_are_passed_over() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        // A WebP is no icon and text is no image, so the third one is kept.
        serve(
            &api,
            Kind::Icon,
            &[(1, b"RIFF\x24\0\0\0WEBPVP8 "), (2, b"hello"), (3, JPEG)],
        );
        let fetched = fetch(&paths, &api, None, Kind::Icon, Mode::Fill).unwrap();
        assert!(
            matches!(fetched, Fetched::Stored { asset: 3, .. }),
            "{fetched:?}"
        );

        serve(&api, Kind::Grid, &[]);
        assert_eq!(
            fetch(&paths, &api, None, Kind::Grid, Mode::Fill).unwrap(),
            Fetched::NoneFound
        );
        serve(&api, Kind::Hero, &[(4, b"hello")]);
        assert_eq!(
            fetch(&paths, &api, None, Kind::Hero, Mode::Fill).unwrap(),
            Fetched::NoneFound
        );
    }

    struct Broken(usize);

    impl Read for Broken {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "connection lost",
                ));
            }
            let n = self.0.min(buf.len());
            buf[..n].fill(0x89);
            self.0 -= n;
            Ok(n)
        }
    }

    #[test]
    fn an_interrupted_or_oversized_download_leaves_nothing_behind() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        let art = paths.art_dir("hades");
        manual(&paths, "hero.png", b"mine");
        serve(&api, Kind::Hero, &[(1, PNG)]);
        api.answer("https://cdn.example/1", 200, || Box::new(Broken(1000)));
        let err = fetch(&paths, &api, None, Kind::Hero, Mode::Replace).unwrap_err();
        assert!(err.to_string().contains("connection lost"), "{err}");
        assert_eq!(names(&art), ["hero.png"]);
        assert_eq!(fs::read(art.join("hero.png")).unwrap(), b"mine");

        api.answer("https://cdn.example/1", 200, || {
            Box::new(io::repeat(0x89).take(crate::art::MAX_BYTES + 1))
        });
        let err = fetch(&paths, &api, None, Kind::Hero, Mode::Replace).unwrap_err();
        assert!(err.to_string().contains("larger than 32 MiB"), "{err}");
        assert_eq!(names(&art), ["hero.png"]);
    }

    #[test]
    fn a_target_fetch_fills_only_its_own_overrides() {
        let (_root, paths) = setup();
        let api = FakeApi::default();
        serve(&api, Kind::Icon, &[(1, PNG)]);
        manual(&paths, "icon.png", b"env");
        let fetched = fetch(&paths, &api, Some("server"), Kind::Icon, Mode::Fill).unwrap();
        assert!(
            matches!(fetched, Fetched::Stored { asset: 1, .. }),
            "{fetched:?}"
        );
        let art = paths.art_dir("hades");
        assert_eq!(fs::read(art.join("server/icon.png")).unwrap(), PNG);
        assert_eq!(fs::read(art.join("icon.png")).unwrap(), b"env");
        let err = fetch(&paths, &api, Some("nope"), Kind::Icon, Mode::Fill).unwrap_err();
        assert!(err.to_string().contains("no target `nope`"), "{err}");
    }

    #[test]
    fn the_game_is_saved_in_art_toml() {
        let (_root, paths) = setup();
        assert_eq!(crate::art::saved_game(&paths, "hades").unwrap(), None);
        crate::art::save_game(&paths, "hades", 5247).unwrap();
        assert_eq!(crate::art::saved_game(&paths, "hades").unwrap(), Some(5247));
        assert!(crate::art::save_game(&paths, "nope", 1).is_err());
    }
}
