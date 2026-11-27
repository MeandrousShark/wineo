use std::io;

use serde_json::json;
use wineo_core::download::Response;

use super::*;
use crate::epic_fake::Epic;
use crate::flow::epic_install::tests::{offline, world};

/// The probe's list with one game another launcher manages, one with no
/// Windows build, one that needs another launcher's account, and box art
/// for the offline game.
fn mixed(epic: &Epic) {
    epic.edit_list(|list| {
        for game in list.as_array_mut().unwrap() {
            match game["app_name"].as_str().unwrap() {
                "AppThirdParty" => {
                    game["metadata"]["customAttributes"]["ThirdPartyManagedApp"] =
                        json!({"type": "STRING", "value": "Origin"});
                }
                "AppToken" => {
                    let windows = game["asset_infos"]["Windows"].take();
                    game["asset_infos"] = json!({"Mac": windows});
                }
                "AppPrereq" => {
                    game["metadata"]["customAttributes"]["partnerLinkType"] =
                        json!({"type": "STRING", "value": "ubisoft"});
                }
                "AppOffline" => {
                    game["metadata"]["keyImages"] = json!([
                        {"type": "DieselGameBoxTall", "url": "https://cdn.example.invalid/tall.png"}
                    ]);
                }
                _ => {}
            }
        }
    });
}

fn listing(epic: &Epic) -> Listing {
    load_listing(&epic.paths, &Token::new()).unwrap()
}

fn tiles(listing: &Listing) -> Vec<(String, Option<String>, Option<String>)> {
    listing
        .games
        .iter()
        .map(|game| {
            let (reason, note) = game.tile_text();
            (game.title.clone(), reason, note)
        })
        .collect()
}

#[test]
fn every_owned_game_shows_with_its_state_or_why_it_cant_install() {
    let epic = world();
    mixed(&epic);
    let listing = listing(&epic);
    assert!(listing.notes.is_empty(), "{:?}", listing.notes);
    let note = |text: &str| Some(text.to_owned());
    assert_eq!(
        tiles(&listing),
        [
            ("Dlc Game".to_owned(), None, note("Not installed")),
            ("Offline Game".to_owned(), None, note("Not installed")),
            (
                "Prereq Game®".to_owned(),
                note("Needs an account at another launcher"),
                None
            ),
            (
                "Third Party Game".to_owned(),
                note("Another launcher manages it"),
                None
            ),
            ("Token Game 2020".to_owned(), note("No Windows build"), None),
        ]
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn installed_unfinished_and_downloaded_games_say_what_wineo_has() {
    let epic = world();
    let paths = &epic.paths;
    let planned = |game: &epic_install::Game| match epic_install::prepare_with(
        paths,
        game,
        &Default::default(),
        &Token::new(),
    )
    .unwrap()
    {
        epic_install::Prepared::Review(review) => review,
        epic_install::Prepared::Offer(_) => unreachable!(),
    };
    let _ = planned;
    // An install canceled partway leaves the game unfinished.
    epic.set_mode("Dlc Game", "hang");
    let dlc = epic_install::Game {
        app: "Dlc Game".to_owned(),
        title: "Dlc Game".to_owned(),
        unfinished: false,
    };
    let review = epic_install::tests::install_review(&epic, &dlc);
    let token = Token::new();
    let (worker_paths, worker_token) = (paths.clone(), token.clone());
    let worker = std::thread::spawn(move || {
        epic_install::install_with(
            &worker_paths,
            &review,
            install::STALL,
            &worker_token,
            |_| {},
            |_, _, _, _| (Vec::new(), Vec::new()),
        )
    });
    epic.wait_started("Dlc Game");
    token.cancel();
    assert!(worker.join().unwrap().is_err());
    let offline_review = epic_install::tests::install_review(&epic, &offline());
    epic_install::install_with(
        paths,
        &offline_review,
        install::STALL,
        &Token::new(),
        |_| {},
        |_, _, _, _| (Vec::new(), Vec::new()),
    )
    .unwrap();
    let shown = tiles(&listing(&epic));
    assert_eq!(shown[0].2.as_deref(), Some("Install didn't finish"));
    assert_eq!(shown[1].2.as_deref(), Some("Installed as offline-game."));
    // Moved to the trash, the game is whole with no environment owning it.
    fs::remove_file(paths.environment_config("offline-game")).unwrap();
    let shown = tiles(&listing(&epic));
    assert_eq!(
        shown[1].1.as_deref(),
        Some("Downloaded, but no environment owns it.")
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn logged_out_or_without_legendary_it_says_why_and_shows_the_saved_list() {
    let epic = Epic::new();
    let listing = listing(&epic);
    assert!(listing.games.is_empty());
    assert_eq!(listing.notes, [LOGGED_OUT]);
    let missing = || {
        Found::Offer(wineo_core::epic::legendary::Missing {
            older_on_path: None,
        })
    };
    let listing = listing_from(&epic.paths, missing(), &Token::new()).unwrap();
    assert_eq!(listing.notes, [LOGGED_OUT]);
    // A list saved earlier shows, with why it may be old.
    epic.log_in();
    let _ = load_listing(&epic.paths, &Token::new()).unwrap();
    let listing = listing_from(&epic.paths, missing(), &Token::new()).unwrap();
    assert_eq!(
        listing.notes,
        ["Legendary isn't installed, so this is the saved list."]
    );
    assert_eq!(listing.games.len(), 5);
    let python = wineo_core::epic::Error::Python.to_string();
    let listing = listing_from(&epic.paths, Found::Unusable(python), &Token::new()).unwrap();
    assert_eq!(
        listing.notes,
        ["Epic needs Python 3.10 or newer to run Legendary. This is the saved list."]
    );
    assert!(epic.sweep().is_empty());
}

#[test]
fn a_portable_session_says_epic_games_live_in_the_host_library() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| std::ffi::OsString::from(&home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    assert_eq!(
        load_listing(&paths, &Token::new()).unwrap_err(),
        "Epic games live in the host library. Run this without --portable."
    );
}

fn page(epic: &Epic) -> Page {
    let (mut page, _) = Page::new(epic.paths.clone());
    let generation = page.generation;
    let listing = listing(epic);
    let _ = page.update(Event::Listed(generation, Ok(listing)));
    page
}

#[test]
fn picking_opens_an_install_or_the_installed_games_card_and_unavailable_ones_stay_put() {
    let epic = world();
    mixed(&epic);
    epic.set_mode("AppOffline", "");
    let offline_review = epic_install::tests::install_review(&epic, &offline());
    epic_install::install_with(
        &epic.paths,
        &offline_review,
        install::STALL,
        &Token::new(),
        |_| {},
        |_, _, _, _| (Vec::new(), Vec::new()),
    )
    .unwrap();
    let mut page = page(&epic);
    // Third Party Game.
    let _ = page.update(Event::Open(3));
    assert!(!page.installing_game());
    assert!(page.take_action().is_none());
    let _ = page.update(Event::Open(1));
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { name: Some(name), launch: false }) if name == "offline-game"
    ));
    let _ = page.update(Event::Open(0));
    assert!(page.installing_game());
    assert!(page.waiting(), "it checks the game with Epic");
    // Back from the review returns to the grid, listed again.
    let _ = page.back();
    assert!(!page.installing_game());
    assert!(page.listing().is_none());
    // And Back from the grid returns to Add.
    let _ = page.back();
    assert!(matches!(page.take_action(), Some(Action::Choose)));
    assert!(epic.sweep().is_empty());
}

#[test]
fn the_grid_dims_what_isnt_installed_and_names_each_state() {
    let epic = world();
    mixed(&epic);
    let page = page(&epic);
    let shown = crate::layout_probe::texts(
        page.view_at(Palette::DARK, 900.0),
        iced::Size::new(900.0, 1400.0),
    );
    for line in [
        "Choose an Epic game",
        "Dlc Game",
        "Not installed",
        "Another launcher\nmanages it",
        "Needs an account\nat another launcher",
        "No Windows build",
    ] {
        assert!(shown.iter().any(|text| text == line), "{line}: {shown:?}");
    }
    let listing = page.listing().unwrap();
    assert!(listing.games.iter().all(|game| !game.installed()));
    assert!(epic.sweep().is_empty());
}

/// Answers each address from a table, counting what was asked.
struct Images(std::sync::Mutex<Vec<String>>, Vec<u8>);

impl Transport for Images {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn io::Read>> {
        self.0.lock().unwrap().push(url.to_owned());
        Ok(Box::new(io::Cursor::new(self.1.clone())))
    }

    fn request(&self, url: &str, _bearer: Option<&str>) -> anyhow::Result<Response> {
        Ok(Response {
            status: 200,
            body: self.get(url)?,
        })
    }
}

fn png() -> Vec<u8> {
    let mut bytes = io::Cursor::new(Vec::new());
    ::image::RgbaImage::from_pixel(4, 6, ::image::Rgba([10, 20, 30, 255]))
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn posters_come_from_epics_box_art_once_and_a_game_without_any_keeps_its_text() {
    let epic = world();
    mixed(&epic);
    let listing = listing(&epic);
    let jobs: Vec<Job> = listing
        .games
        .iter()
        .map(|game| Job {
            id: game.app.clone(),
            source: Source::Cover(game.images.clone()),
        })
        .collect();
    let asked = std::sync::Arc::new(Images(Default::default(), png()));
    let mut posters = Vec::new();
    let transport = std::sync::Arc::clone(&asked);
    run_posters(
        &epic.paths,
        jobs.clone(),
        &Token::new(),
        move || Ok(Box::new(Shared(transport))),
        |id, poster| posters.push((id, poster.is_ok())),
    );
    assert!(
        posters.contains(&("AppOffline".to_owned(), true)),
        "{posters:?}"
    );
    assert_eq!(
        posters.iter().filter(|(_, ok)| *ok).count(),
        1,
        "only the offline game has box art: {posters:?}"
    );
    assert_eq!(asked.0.lock().unwrap().len(), 1);
    // The second load reads the cache and never opens the network.
    let mut again = Vec::new();
    run_posters(
        &epic.paths,
        jobs,
        &Token::new(),
        || Err("no network".to_owned()),
        |id, poster| again.push((id, poster.is_ok())),
    );
    assert!(
        again.contains(&("AppOffline".to_owned(), true)),
        "{again:?}"
    );
    assert_eq!(asked.0.lock().unwrap().len(), 1);
    assert!(epic.sweep().is_empty());
}

struct Shared(std::sync::Arc<Images>);

impl Transport for Shared {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn io::Read>> {
        self.0.get(url)
    }

    fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
        self.0.request(url, bearer)
    }
}

use std::fs;
