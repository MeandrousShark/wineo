use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Paths, Page) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let config = paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        "schema_version = 1\nkind = 'native'\n[targets.alt]\nexe = 'game.exe'\n",
    )
    .unwrap();
    let identity = EnvironmentIdentity::read(&paths, "game").unwrap();
    (root, paths, Page::new("game".into(), 1, identity))
}

fn decodes() -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(2))
}

fn snapshot(page: &mut Page, paths: &Paths) {
    page.load_sequence = page.load_sequence.wrapping_add(1);
    let state = load_snapshot(paths, &page.name).unwrap();
    let _ = page.update(
        paths,
        Event::Loaded(page.load_sequence, Ok(state)),
        decodes(),
    );
}

fn png(root: &Path) -> PathBuf {
    let path = root.join("image.png");
    ::image::RgbaImage::from_pixel(8, 12, ::image::Rgba([255, 0, 0, 255]))
        .save_with_format(&path, ::image::ImageFormat::Png)
        .unwrap();
    path
}

#[test]
fn game_label_uses_name_or_saved_id_and_handles_no_pick() {
    let (_root, _paths, mut page) = fixture();
    assert_eq!(page.game_label(false), "Not picked");
    page.game_id = Some(123);
    assert_eq!(page.game_label(false), "123");
    page.game = Some(Game {
        id: 123,
        name: "Hades".into(),
        year: Some(2020),
    });
    assert_eq!(page.game_label(false), "123");
    assert_eq!(page.game_label(true), "Hades (2020)");
    page.game_id = Some(456);
    assert_eq!(page.game_label(true), "456");
}

#[test]
fn change_action_uses_app_key_availability() {
    let (_root, paths, mut page) = fixture();
    let _ = page.update(&paths, Event::Change, decodes());
    assert!(page.change.is_none());
    let _ = page.update_with_key_availability(&paths, Event::Change, decodes(), true);
    assert!(page.change.is_some());
}

#[test]
fn failed_lookup_keeps_id_and_can_retry_on_next_load() {
    let (_root, paths, mut page) = fixture();
    page.game_id = Some(123);
    let _ = page.lookup_game(&paths, true);
    let first = page.lookup_sequence;
    assert_eq!(page.looked_up, Some(123));
    let _ = page.update_with_key_availability(
        &paths,
        Event::GameLoaded(first, 123, Err("offline".into())),
        decodes(),
        true,
    );
    assert_eq!(page.game_label(true), "123");
    assert_eq!(page.looked_up, None);
    let _ = page.lookup_game(&paths, true);
    assert!(page.lookup_sequence > first);
}

#[test]
fn stale_game_lookup_cannot_replace_a_saved_match() {
    let (_root, paths, mut page) = fixture();
    page.game_id = Some(123);
    let _ = page.lookup_game(&paths, true);
    let old = page.lookup_sequence;
    page.lookup_sequence += 1;
    page.game = Some(Game {
        id: 123,
        name: "Picked".into(),
        year: None,
    });
    let _ = page.update_with_key_availability(
        &paths,
        Event::GameLoaded(
            old,
            123,
            Ok(Some(Game {
                id: 123,
                name: "Stale".into(),
                year: None,
            })),
        ),
        decodes(),
        true,
    );
    assert_eq!(page.game_label(true), "Picked");
}

#[test]
fn manual_art_sets_environment_and_target_then_inherits_again() {
    let (root, paths, mut page) = fixture();
    let source = png(root.path());
    let env_spot = (None, Kind::Grid);
    let target_spot = (Some("alt".into()), Kind::Grid);
    let wrote = write_art(&paths, "game", &env_spot, Some(&source));
    assert!(wrote.changed && wrote.error.is_none());
    snapshot(&mut page, &paths);
    assert!(page.has_override(Kind::Grid));
    let label = source_label(page.rows.get(&env_spot).unwrap());
    assert_eq!(label, "Manual");
    let _ = page.update(
        &paths,
        Event::Showing(Showing(Some("alt".into()))),
        decodes(),
    );
    assert!(page.inherited(Kind::Grid));
    assert!(!page.has_override(Kind::Grid));
    let wrote = write_art(&paths, "game", &target_spot, Some(&source));
    assert!(wrote.changed && wrote.error.is_none());
    snapshot(&mut page, &paths);
    assert!(!page.inherited(Kind::Grid));
    assert!(page.has_override(Kind::Grid));
    let wrote = write_art(&paths, "game", &target_spot, None);
    assert!(wrote.changed && wrote.error.is_none());
    snapshot(&mut page, &paths);
    assert!(page.inherited(Kind::Grid));
    let wrote = write_art(&paths, "game", &env_spot, None);
    assert!(wrote.changed && wrote.error.is_none());
    snapshot(&mut page, &paths);
    assert!(!page.has_override(Kind::Grid));
}

#[test]
fn browse_and_remove_events_update_target_override_and_environment() {
    let (root, paths, mut page) = fixture();
    let source = png(root.path());
    let env_spot = (None, Kind::Grid);
    let target_spot = (Some("alt".into()), Kind::Grid);
    snapshot(&mut page, &paths);

    let _ = page.update(&paths, Event::Browse(Kind::Grid), decodes());
    let chooser = page.chooser_sequence;
    let _ = page.update(
        &paths,
        Event::Picked(chooser, env_spot.clone(), Ok(Some(source.clone()))),
        decodes(),
    );
    assert!(page.busy());
    let outcome = write_art(&paths, "game", &env_spot, Some(&source));
    let step = page.update(
        &paths,
        Event::Wrote(page.write_sequence, env_spot.clone(), outcome),
        decodes(),
    );
    assert!(step.changed);
    assert!(!page.busy());
    snapshot(&mut page, &paths);
    assert!(page.has_override(Kind::Grid));

    let _ = page.update(
        &paths,
        Event::Showing(Showing(Some("alt".into()))),
        decodes(),
    );
    assert!(page.inherited(Kind::Grid));
    let _ = page.update(&paths, Event::Browse(Kind::Grid), decodes());
    let chooser = page.chooser_sequence;
    let _ = page.update(
        &paths,
        Event::Picked(chooser, target_spot.clone(), Ok(Some(source.clone()))),
        decodes(),
    );
    assert!(page.busy());
    let outcome = write_art(&paths, "game", &target_spot, Some(&source));
    let step = page.update(
        &paths,
        Event::Wrote(page.write_sequence, target_spot.clone(), outcome),
        decodes(),
    );
    assert!(step.changed);
    snapshot(&mut page, &paths);
    assert!(!page.inherited(Kind::Grid));
    assert!(page.has_override(Kind::Grid));

    let _ = page.update(&paths, Event::Remove(Kind::Grid), decodes());
    assert!(page.busy());
    let outcome = write_art(&paths, "game", &target_spot, None);
    let step = page.update(
        &paths,
        Event::Wrote(page.write_sequence, target_spot, outcome),
        decodes(),
    );
    assert!(step.changed);
    snapshot(&mut page, &paths);
    assert!(page.inherited(Kind::Grid));
    assert!(!page.has_override(Kind::Grid));

    let _ = page.update(&paths, Event::Showing(Showing(None)), decodes());
    let _ = page.update(&paths, Event::Remove(Kind::Grid), decodes());
    assert!(page.busy());
    let outcome = write_art(&paths, "game", &env_spot, None);
    let step = page.update(
        &paths,
        Event::Wrote(page.write_sequence, env_spot, outcome),
        decodes(),
    );
    assert!(step.changed);
    snapshot(&mut page, &paths);
    assert!(!page.has_override(Kind::Grid));
    assert!(
        art::list(&paths, "game")
            .unwrap()
            .kinds
            .iter()
            .all(|(kind, resolved)| *kind != Kind::Grid || resolved.is_none())
    );
}

#[test]
fn invalid_content_is_an_error_on_its_kind_row() {
    let (root, paths, mut page) = fixture();
    let text_file = root.path().join("wrong.png");
    fs::write(&text_file, b"not a png").unwrap();
    page.chooser_sequence = 1;
    page.chooser_active = true;
    let _ = page.update(
        &paths,
        Event::Picked(1, (None, Kind::Grid), Ok(Some(text_file.clone()))),
        decodes(),
    );
    assert!(page.busy());
    let outcome = write_art(&paths, "game", &(None, Kind::Grid), Some(&text_file));
    assert!(!outcome.changed);
    let _ = page.update(
        &paths,
        Event::Wrote(page.write_sequence, (None, Kind::Grid), outcome),
        decodes(),
    );
    assert!(!page.busy());
    assert!(page.row_errors.contains_key(&(None, Kind::Grid)));
}

#[test]
fn chooser_result_after_leaving_cannot_start_a_write() {
    let (_root, paths, mut page) = fixture();
    let _ = page.update(&paths, Event::Browse(Kind::Hero), decodes());
    let sequence = page.chooser_sequence;
    page.leave();
    let _ = page.update(
        &paths,
        Event::Picked(
            sequence,
            (None, Kind::Hero),
            Ok(Some(PathBuf::from("/tmp/a.png"))),
        ),
        decodes(),
    );
    assert!(!page.busy());
    assert!(page.change.is_none());
}

#[test]
fn missing_key_disables_change_and_find() {
    let (_root, paths, mut page) = fixture();
    let _ = page.update(&paths, Event::Change, decodes());
    assert!(page.change.is_none());
    let _ = page.update(&paths, Event::SaveGame, decodes());
    assert!(!page.busy());
}

#[test]
fn changed_game_saves_and_uses_its_name_immediately() {
    let (_root, paths, mut page) = fixture();
    let game = Game {
        id: 77,
        name: "Celeste".into(),
        year: Some(2018),
    };
    let _ = page.update_with_key_availability(&paths, Event::Change, decodes(), true);
    page.change.as_mut().unwrap().selected = Some(Match(game.clone()));
    let _ = page.update_with_key_availability(&paths, Event::SaveGame, decodes(), true);
    assert!(page.busy());
    art::save_game(&paths, "game", game.id).unwrap();
    let step = page.update(
        &paths,
        Event::GameSaved(page.write_sequence, game, Ok(())),
        decodes(),
    );
    assert!(step.changed);
    assert!(!page.busy());
    assert_eq!(page.game_label(true), "Celeste (2018)");
    assert_eq!(art::saved_game(&paths, "game").unwrap(), Some(77));
}

#[test]
fn preview_reports_original_pixel_size() {
    let (root, _paths, _page) = fixture();
    let path = png(root.path());
    let (_, dimensions) = decode(&path, Kind::Grid, root.path()).unwrap();
    assert_eq!(dimensions, Some((8, 12)));
}

#[test]
fn preview_reports_svg_pixel_size() {
    let (root, _paths, _page) = fixture();
    let path = root.path().join("hero.svg");
    fs::write(
        &path,
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="620"><rect width="1920" height="620" fill="red"/></svg>"#,
    )
    .unwrap();
    let (_, dimensions) = decode(&path, Kind::Hero, root.path()).unwrap();
    assert_eq!(dimensions, Some((1920, 620)));
}

#[test]
fn reload_picks_up_art_changed_outside_the_page() {
    let (root, paths, mut page) = fixture();
    snapshot(&mut page, &paths);
    assert!(!page.has_override(Kind::Grid));

    let source = png(root.path());
    art::set_manual(&paths, "game", None, Kind::Grid, &source, None).unwrap();
    snapshot(&mut page, &paths);

    assert!(page.has_override(Kind::Grid));
}

#[test]
fn late_lookup_and_preview_results_are_discarded_after_leaving() {
    let (root, paths, mut page) = fixture();
    page.game_id = Some(123);
    let _ = page.lookup_game(&paths, true);
    let lookup_sequence = page.lookup_sequence;
    page.leave();
    let _ = page.update(
        &paths,
        Event::GameLoaded(
            lookup_sequence,
            123,
            Ok(Some(Game {
                id: 123,
                name: "Stale".into(),
                year: None,
            })),
        ),
        decodes(),
    );
    assert_eq!(page.game_label(true), "123");

    let source = png(root.path());
    art::set_manual(&paths, "game", None, Kind::Grid, &source, None).unwrap();
    snapshot(&mut page, &paths);
    let sequence = page.load_sequence;
    let spot = (None, Kind::Grid);
    page.leave();
    let _ = page.update(
        &paths,
        Event::Previewed(
            sequence,
            spot.clone(),
            Ok((image::Handle::from_rgba(1, 1, vec![0; 4]), Some((8, 12)))),
        ),
        decodes(),
    );
    assert!(page.rows.get(&spot).unwrap().preview.is_none());
}

#[test]
fn changing_icon_art_reconciles_the_desktop_entry() {
    let (root, paths, _page) = fixture();
    fs::write(
        paths.environment_config("game"),
        "schema_version = 1\nkind = 'native'\n[targets.alt]\nexe = 'notepad'\ndesktop = true\n",
    )
    .unwrap();
    let source = root.path().join("icon.png");
    ::image::RgbaImage::new(64, 64)
        .save_with_format(&source, ::image::ImageFormat::Png)
        .unwrap();

    let outcome = write_art(&paths, "game", &(None, Kind::Icon), Some(&source));

    assert!(outcome.changed, "{outcome:?}");
    assert!(outcome.error.is_none(), "{outcome:?}");
    let entry = paths.applications_dir().join("wineo-game-alt.desktop");
    assert!(entry.is_file());
    let contents = fs::read_to_string(entry).unwrap();
    assert!(contents.contains("Icon=wineo-game-alt\n"));
}

#[test]
fn a_queued_write_refuses_a_same_name_environment_replacement() {
    let (root, paths, page) = fixture();
    let identity = page.identity;
    let retained = root.path().join("retained-game.toml");
    fs::rename(paths.environment_config("game"), retained).unwrap();
    wineo_core::environment::Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
    )
    .unwrap();
    let source = png(root.path());

    let outcome = write_art_for(&paths, "game", identity, &(None, Kind::Grid), Some(&source));

    assert!(!outcome.changed());
    assert!(outcome.error.is_some());
    assert!(
        art::resolve(
            &paths.art_dir("game"),
            &wineo_core::art::ArtConfig::load(&paths.art_dir("game")).unwrap(),
            None,
            Kind::Grid,
        )
        .unwrap()
        .is_none()
    );
}

struct ImageApi {
    bytes: Vec<u8>,
    urls: std::cell::RefCell<Vec<String>>,
}

impl wineo_core::download::Transport for ImageApi {
    fn get(&self, _url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        anyhow::bail!("unexpected plain request")
    }

    fn request(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> anyhow::Result<wineo_core::download::Response> {
        assert!(bearer.is_none());
        self.urls.borrow_mut().push(url.into());
        Ok(wineo_core::download::Response {
            status: 200,
            body: Box::new(std::io::Cursor::new(self.bytes.clone())),
        })
    }
}

fn chosen_image() -> steamgriddb::Image {
    steamgriddb::Image {
        id: 914,
        url: "https://cdn.example/chosen.png".into(),
        thumb: Some("https://cdn.example/thumbnail.png".into()),
        ..Default::default()
    }
}

#[test]
fn picker_use_publishes_exact_image_and_refreshes_the_kind() {
    let (root, paths, mut page) = fixture();
    let source = png(root.path());
    let transport = ImageApi {
        bytes: fs::read(&source).unwrap(),
        urls: Default::default(),
    };
    let image = chosen_image();
    let spot = (Some("alt".into()), Kind::Grid);
    let outcome = use_image_for(
        &paths,
        "game",
        page.identity,
        &spot,
        &Api::new(&transport, "key"),
        &image,
        |_, _| {},
        &Token::new(),
    );
    assert!(outcome.changed(), "{outcome:?}");
    assert_eq!(*transport.urls.borrow(), vec![image.url]);
    page.writing = true;
    page.write_sequence = 4;
    let step = page.update(
        &paths,
        Event::Used(4, page.picker_sequence, spot.clone(), outcome),
        decodes(),
    );
    assert!(step.changed);
    assert!(!page.busy());
    snapshot(&mut page, &paths);
    assert_eq!(
        page.rows[&spot].resolved.as_ref().unwrap().source,
        Some(Source::SteamGridDb(914))
    );
}

#[test]
fn picker_cancel_keeps_prior_art_and_refused_format_keeps_cores_message() {
    let (root, paths, page) = fixture();
    let source = png(root.path());
    art::set_manual(&paths, "game", None, Kind::Grid, &source, None).unwrap();
    let before = fs::read(paths.art_dir("game").join("grid.png")).unwrap();
    let image = chosen_image();
    let transport = ImageApi {
        bytes: before.clone(),
        urls: Default::default(),
    };
    let token = Token::new();
    let outcome = use_image_for(
        &paths,
        "game",
        page.identity,
        &(None, Kind::Grid),
        &Api::new(&transport, "key"),
        &image,
        |_, _| token.cancel(),
        &token,
    );
    assert!(!outcome.changed());
    assert!(outcome.error.unwrap().contains("cancel"));
    assert_eq!(
        fs::read(paths.art_dir("game").join("grid.png")).unwrap(),
        before
    );
    let transport = ImageApi {
        bytes: b"not an image".to_vec(),
        urls: Default::default(),
    };
    let outcome = use_image_for(
        &paths,
        "game",
        page.identity,
        &(None, Kind::Grid),
        &Api::new(&transport, "key"),
        &image,
        |_, _| {},
        &Token::new(),
    );
    assert!(!outcome.changed());
    assert!(outcome.error.unwrap().starts_with("asset 914 is not a "));
    assert_eq!(
        fs::read(paths.art_dir("game").join("grid.png")).unwrap(),
        before
    );
}

#[test]
fn picker_use_icon_reconciles_desktop_entries() {
    let (root, paths, page) = fixture();
    fs::write(
        paths.environment_config("game"),
        "schema_version = 1\nkind = 'native'\n[targets.alt]\nexe = 'notepad'\ndesktop = true\n",
    )
    .unwrap();
    assert_eq!(
        EnvironmentIdentity::read(&paths, "game"),
        Some(page.identity)
    );
    let source = png(root.path());
    let transport = ImageApi {
        bytes: fs::read(&source).unwrap(),
        urls: Default::default(),
    };
    let outcome = use_image_for(
        &paths,
        "game",
        page.identity,
        &(None, Kind::Icon),
        &Api::new(&transport, "key"),
        &chosen_image(),
        |_, _| {},
        &Token::new(),
    );
    assert!(outcome.changed(), "{outcome:?}");
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert!(
        paths
            .applications_dir()
            .join("wineo-game-alt.desktop")
            .exists()
    );
}

#[test]
fn find_without_a_game_routes_through_change_then_opens_picker() {
    let (_root, paths, mut page) = fixture();
    let _ = page.update_with_key_availability(&paths, Event::Find(Kind::Hero), decodes(), true);
    assert!(page.picker.is_none());
    assert!(page.change.is_some());
    assert_eq!(page.pending_find, Some(Kind::Hero));
    let game = Game {
        id: 9,
        name: "Hades".into(),
        year: None,
    };
    page.change.as_mut().unwrap().selected = Some(Match(game.clone()));
    let _ = page.update_with_key_availability(&paths, Event::SaveGame, decodes(), true);
    art::save_game(&paths, "game", game.id).unwrap();
    let _ = page.update_with_key_availability(
        &paths,
        Event::GameSaved(page.write_sequence, game, Ok(())),
        decodes(),
        true,
    );
    assert!(page.change.is_none());
    assert_eq!(page.picker.as_ref().unwrap().kind, Kind::Hero);
    assert_eq!(page.pending_find, None);
}

#[test]
fn leaving_picker_retains_download_and_ignores_late_listing() {
    let (_root, paths, mut page) = fixture();
    page.game_id = Some(9);
    let _ = page.update_with_key_availability(&paths, Event::Find(Kind::Grid), decodes(), true);
    let visit = page.picker_sequence;
    page.writing = true;
    let token = Token::new();
    page.download = Some(Download {
        spot: (None, Kind::Grid),
        token: token.clone(),
        status: "Downloading...".into(),
    });
    let _ = page.update(
        &paths,
        Event::Picker(visit, art_picker::Event::Back),
        decodes(),
    );
    assert!(page.picker.is_none());
    assert!(page.busy());
    assert!(!token.is_canceled());
    page.leave();
    assert!(page.busy());
    assert!(!token.is_canceled());
    let _ = page.update(
        &paths,
        Event::Picker(
            visit,
            art_picker::Event::Listed(1, Ok(vec![chosen_image()])),
        ),
        decodes(),
    );
    assert!(page.picker.is_none());
    let _ = page.update(&paths, Event::CancelUse, decodes());
    assert!(token.is_canceled());
    assert!(page.busy());
}

fn load_picker(page: &mut Page, paths: &Paths) -> u64 {
    page.game_id = Some(9);
    let _ = page.update_with_key_availability(paths, Event::Find(Kind::Grid), decodes(), true);
    let visit = page.picker_sequence;
    let sequence = page.picker.as_ref().unwrap().list_sequence();
    let images = (1..=24)
        .map(|id| steamgriddb::Image {
            id,
            width: Some(600),
            height: Some(900),
            ..chosen_image()
        })
        .collect();
    let _ = page.update(
        paths,
        Event::Picker(visit, art_picker::Event::Listed(sequence, Ok(images))),
        decodes(),
    );
    visit
}

#[test]
fn picker_keyboard_moves_and_use_targets_the_selected_image() {
    let (_root, paths, mut page) = fixture();
    let visit = load_picker(&mut page, &paths);
    for (direction, id) in [
        (crate::keys::Direction::Right, 1),
        (crate::keys::Direction::Right, 2),
        (crate::keys::Direction::Down, 7),
        (crate::keys::Direction::Left, 6),
        (crate::keys::Direction::Up, 1),
    ] {
        let _ = page.update(
            &paths,
            Event::Picker(visit, art_picker::Event::Move(direction)),
            decodes(),
        );
        assert_eq!(page.picker.as_ref().unwrap().selected().unwrap().id, id);
    }
    let _ = page.update(
        &paths,
        Event::Picker(visit, art_picker::Event::UseSelected(7)),
        decodes(),
    );
    assert!(page.busy());
    assert_eq!(page.picker.as_ref().unwrap().selected().unwrap().id, 8);
    let outcome = WriteOutcome::failed("asset 8 is not a PNG or JPEG image".into());
    let _ = page.update(
        &paths,
        Event::Used(page.write_sequence, visit, (None, Kind::Grid), outcome),
        decodes(),
    );
    assert_eq!(
        page.picker.as_ref().unwrap().error.as_deref(),
        Some("asset 8 is not a PNG or JPEG image")
    );
    assert!(!page.busy());
}

#[test]
fn picker_session_cache_survives_back_and_next_visit_resets_filters() {
    let (_root, paths, mut page) = fixture();
    let visit = load_picker(&mut page, &paths);
    let url = "https://cdn.example/cached.png";
    page.thumbnails.lock().unwrap().insert(
        Kind::Grid,
        url.into(),
        image::Handle::from_rgba(1, 1, vec![0; 4]),
    );
    let _ = page.update(
        &paths,
        Event::Picker(
            visit,
            art_picker::Event::Filter(art_picker::Filter::Animated, true),
        ),
        decodes(),
    );
    let _ = page.update(&paths, Event::Back, decodes());
    let next_visit = load_picker(&mut page, &paths);
    assert_ne!(visit, next_visit);
    assert!(page.thumbnails.lock().unwrap().contains(Kind::Grid, url));
    assert_eq!(page.picker.as_ref().unwrap().list_sequence(), 1);
}

#[test]
fn reload_after_saved_game_changes_discards_the_old_picker() {
    let (_root, paths, mut page) = fixture();
    let visit = load_picker(&mut page, &paths);
    snapshot(&mut page, &paths);
    assert!(page.picker.is_none());
    let _ = page.update(
        &paths,
        Event::Picker(
            visit,
            art_picker::Event::Filter(art_picker::Filter::Animated, true),
        ),
        decodes(),
    );
    assert!(page.picker.is_none());
    assert!(!page.busy());
}

#[test]
fn picker_use_refuses_replacement_in_the_last_progress_callback() {
    let (root, paths, page) = fixture();
    let source = png(root.path());
    let bytes = fs::read(&source).unwrap();
    let transport = ImageApi {
        bytes: bytes.clone(),
        urls: Default::default(),
    };
    let replacement = root.path().join("replacement.png");
    ::image::RgbaImage::from_pixel(8, 12, ::image::Rgba([0, 0, 255, 255]))
        .save(&replacement)
        .unwrap();
    let mut replaced = false;
    let outcome = use_image_for(
        &paths,
        "game",
        page.identity,
        &(None, Kind::Grid),
        &Api::new(&transport, "key"),
        &chosen_image(),
        |received, _| {
            if received == bytes.len() as u64 && !replaced {
                fs::rename(
                    paths.environment_config("game"),
                    root.path().join("old-game.toml"),
                )
                .unwrap();
                Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false)
                    .unwrap();
                art::set_manual(&paths, "game", None, Kind::Grid, &replacement, None).unwrap();
                replaced = true;
            }
        },
        &Token::new(),
    );
    assert!(replaced);
    assert!(!outcome.changed());
    assert!(outcome.error.unwrap().contains("environment"));
    assert_eq!(
        fs::read(paths.art_dir("game").join("grid.png")).unwrap(),
        fs::read(replacement).unwrap()
    );
    assert_eq!(
        art::ArtConfig::load(&paths.art_dir("game"))
            .unwrap()
            .record(None, Kind::Grid)
            .unwrap()
            .source,
        Source::Manual("replacement.png".into())
    );
}

#[test]
fn reopening_another_kind_abandons_queued_thumbnails() {
    let (root, paths, mut page) = fixture();
    let visit = load_picker(&mut page, &paths);
    let old_token = page.picker.as_ref().unwrap().thumbnail_token();
    let bytes = fs::read(png(root.path())).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let decodes = Arc::new(tokio::sync::Semaphore::new(1));
        let held = decodes.clone().acquire_owned().await.unwrap();
        let mut old_tasks = Vec::new();
        for _ in 0..3 {
            let (sent, fetched) = tokio::sync::oneshot::channel();
            let bytes = bytes.clone();
            old_tasks.push(tokio::spawn(art_picker::thumbnail_work(
                move || {
                    sent.send(()).unwrap();
                    Ok(bytes)
                },
                Kind::Grid,
                decodes.clone(),
                old_token.clone(),
            )));
            fetched.await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(old_tasks.iter().all(|task| !task.is_finished()));

        let _ = page.update_with_key_availability(
            &paths,
            Event::Find(Kind::Hero),
            decodes.clone(),
            true,
        );
        assert!(old_token.is_canceled());
        assert_eq!(page.picker.as_ref().unwrap().kind, Kind::Hero);
        let new_token = page.picker.as_ref().unwrap().thumbnail_token();
        let (sent, fetched) = tokio::sync::oneshot::channel();
        let current = tokio::spawn(art_picker::thumbnail_work(
            move || {
                sent.send(()).unwrap();
                Ok(bytes)
            },
            Kind::Hero,
            decodes.clone(),
            new_token,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), fetched)
            .await
            .unwrap()
            .unwrap();
        for task in old_tasks {
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(2), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(decodes.available_permits(), 0);
        assert!(!current.is_finished());
        drop(held);
        let handle = tokio::time::timeout(std::time::Duration::from_secs(2), current)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(decodes.available_permits(), 1);

        let _ = page.update(
            &paths,
            Event::Picker(
                visit,
                art_picker::Event::Thumbnail(1, "old".into(), Some(Ok(handle))),
            ),
            decodes,
        );
        assert!(!page.thumbnails.lock().unwrap().contains(Kind::Hero, "old"));
        assert!(!page.thumbnails.lock().unwrap().contains(Kind::Grid, "old"));
    });
    let token = page.picker.as_ref().unwrap().thumbnail_token();
    page.leave();
    assert!(token.is_canceled());
}

#[test]
fn browse_write_rechecks_identity_after_art_lock_contention() {
    let (root, paths, page) = fixture();
    let source = png(root.path());
    art::set_manual(&paths, "game", None, Kind::Grid, &source, None).unwrap();
    let art_dir = paths.art_dir("game");
    let held = wineo_core::operation::Locks::new(&paths)
        .config(&art_dir.join(art::ART_FILE))
        .unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let paths = paths.clone();
        move || {
            started.send(()).unwrap();
            write_art_for(
                &paths,
                "game",
                page.identity,
                &(None, Kind::Grid),
                Some(&source),
            )
        }
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(!worker.is_finished());
    let config = paths.environment_config("game");
    let replacement = config.with_extension("replacement");
    fs::write(&replacement, fs::read(&config).unwrap()).unwrap();
    fs::rename(replacement, config).unwrap();
    ::image::RgbaImage::from_pixel(8, 12, ::image::Rgba([0, 0, 255, 255]))
        .save(art_dir.join("grid.png"))
        .unwrap();
    let saved_art = fs::read(art_dir.join("grid.png")).unwrap();
    let saved_record = fs::read(art_dir.join(art::ART_FILE)).unwrap();
    let mut saved_files: Vec<_> = fs::read_dir(&art_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    saved_files.sort();
    drop(held);

    let outcome = worker.join().unwrap();
    assert!(!outcome.changed());
    assert!(outcome.error.unwrap().contains("changed"));
    assert_eq!(fs::read(art_dir.join("grid.png")).unwrap(), saved_art);
    assert_eq!(fs::read(art_dir.join(art::ART_FILE)).unwrap(), saved_record);
    let mut files: Vec<_> = fs::read_dir(&art_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(files, saved_files);
}

/// Environment plus three long target names is the most the toggle row
/// shows before it becomes a pick list.
#[test]
fn showing_choices_wrap_without_spilling_or_squeezing() {
    use crate::layout_probe::{layout, probe};
    let (_root, _paths, mut page) = fixture();
    page.targets = vec![
        "a-rather-long-target-name".to_owned(),
        "another-quite-long-target-name".to_owned(),
        "yet-another-long-target-name".to_owned(),
    ];
    let widths = |width: f32| -> Vec<f32> {
        let mut view = page.showing_view(Palette::DARK);
        let node = layout(&mut view, iced::Size::new(width, 400.0));
        node.children().iter().map(|c| c.size().width).collect()
    };
    let natural = widths(4000.0);
    assert_eq!(natural.len(), 5, "label plus four choices");
    // Content widths: the page's 24 px padding comes off the window width.
    for width in [360.0_f32 - 48.0, 860.0 - 48.0] {
        assert_eq!(widths(width), natural, "{width}: a choice lost its width");
        let mut view = page.showing_view(Palette::DARK);
        let frame = iced::Rectangle::with_size(iced::Size::new(width, 400.0));
        let spilled = probe(&mut view, frame.size()).outside(frame);
        assert!(spilled.is_empty(), "{width}: {spilled:?}");
    }
}
