use std::fs;
use std::io;

use super::*;
use crate::epic::fake::Setup;
use crate::steamgriddb::fake::FakeApi;

const APP: &str = "AppOffline";
const POSTER: &str = "https://cdn.example.invalid/tall.png";
const HERO: &str = "https://cdn.example.invalid/wide.png";

fn png() -> Vec<u8> {
    let mut bytes = io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 6, image::Rgba([10, 20, 30, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn images() -> Vec<Image> {
    [
        ("DieselGameBoxTall", POSTER),
        ("DieselGameBox", HERO),
        ("Thumbnail", "https://cdn.example.invalid/thumb.png"),
    ]
    .map(|(kind, url)| Image {
        kind: kind.to_owned(),
        url: url.to_owned(),
    })
    .to_vec()
}

fn environment(setup: &Setup, name: &str, epic_id: &str) {
    let config = setup.paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "prefix = \"/tmp/{name}\"\nrunner = \"system\"\narch = \"win64\"\nepic_id = \"{epic_id}\"\n"
        ),
    )
    .unwrap();
}

fn request<'a>(name: &'a str, images: &'a [Image], transport: &'a FakeApi) -> Request<'a> {
    Request {
        name,
        app_name: APP,
        images,
        kinds: &Kind::ALL,
        transport,
    }
}

#[test]
fn each_kind_takes_its_own_epic_image() {
    let images = images();
    assert_eq!(link(&images, Kind::Grid), Some(POSTER));
    assert_eq!(link(&images, Kind::Hero), Some(HERO));
    assert_eq!(link(&images, Kind::Logo), None);
    assert_eq!(link(&images, Kind::Icon), None, "Epic has no icon");
}

#[test]
fn epic_art_fills_what_is_missing_and_is_recorded_as_epic() {
    let setup = Setup::new();
    environment(&setup, "fowl", APP);
    let transport = FakeApi::default();
    transport.bytes(POSTER, &png());
    transport.bytes(HERO, &png());
    let images = images();
    let request = request("fowl", &images, &transport);
    let report = import(&setup.paths, &request, &Token::new()).unwrap();
    assert_eq!(report.art, [Kind::Grid, Kind::Hero]);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(
        report.line().as_deref(),
        Some("Added Epic art: poster, hero.")
    );
    let text = fs::read_to_string(setup.paths.art_dir("fowl").join("art.toml")).unwrap();
    assert!(text.contains("epic = \"AppOffline\""), "{text}");
    let config = art::ArtConfig::load(&setup.paths.art_dir("fowl")).unwrap();
    assert_eq!(
        config.kinds[&Kind::Grid].source,
        Source::Epic(APP.to_owned())
    );

    // Art it has is kept, and nothing is fetched again.
    let again = import(&setup.paths, &request, &Token::new()).unwrap();
    assert!(again.art.is_empty());
    assert_eq!(again.line(), None);
    assert_eq!(transport.urls(), [POSTER, HERO]);
}

#[test]
fn only_the_environment_running_the_game_gets_its_art() {
    let setup = Setup::new();
    environment(&setup, "other", "AppElse");
    let transport = FakeApi::default();
    let images = images();
    let error = import(
        &setup.paths,
        &request("other", &images, &transport),
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "other doesn't run Epic game AppOffline.");
    assert!(transport.urls().is_empty());
}

#[test]
fn a_failed_image_is_a_note() {
    let setup = Setup::new();
    environment(&setup, "fowl", APP);
    let transport = FakeApi::default();
    transport.bytes(POSTER, b"not an image");
    transport.bytes(HERO, &png());
    let images = images();
    let report = import(
        &setup.paths,
        &request("fowl", &images, &transport),
        &Token::new(),
    )
    .unwrap();
    assert_eq!(report.art, [Kind::Hero]);
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    assert!(
        report.notes[0].starts_with("Couldn't fetch Epic grid artwork:"),
        "{:?}",
        report.notes
    );
}

#[test]
fn art_toml_reads_and_rejects_epic_sources() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("art.toml");
    let parsed = art::ArtConfig::parse(
        &path,
        "schema_version = 1\n[grid]\nfile = \"grid.png\"\nepic = \"Dlc Game\"\n",
    )
    .unwrap();
    assert_eq!(
        parsed.kinds[&Kind::Grid].source,
        Source::Epic("Dlc Game".to_owned())
    );
    for broken in [
        "[grid]\nfile = \"grid.png\"\nepic = \"\"\n",
        "[grid]\nfile = \"grid.png\"\nepic = \"x\"\ngog = 1\n",
    ] {
        let error = format!("{:#}", art::ArtConfig::parse(&path, broken).unwrap_err());
        assert!(
            error.contains("needs exactly one of `steamgriddb`, `manual`, `gog`, and `epic`"),
            "{error}"
        );
    }
}
