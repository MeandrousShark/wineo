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
    assert_eq!(page.game_label(), "Not picked");
    page.game_id = Some(123);
    assert_eq!(page.game_label(), "123");
    page.key_available = false;
    assert_eq!(page.game_label(), "123");
    page.game = Some(Game {
        id: 123,
        name: "Hades".into(),
        year: Some(2020),
    });
    assert_eq!(page.game_label(), "123");
    page.key_available = true;
    assert_eq!(page.game_label(), "Hades (2020)");
    page.game_id = Some(456);
    assert_eq!(page.game_label(), "456");
}

#[test]
fn failed_lookup_keeps_id_and_can_retry_on_next_load() {
    let (_root, paths, mut page) = fixture();
    page.game_id = Some(123);
    page.key_available = true;
    let _ = page.lookup_game(&paths);
    let first = page.lookup_sequence;
    assert_eq!(page.looked_up, Some(123));
    let _ = page.update(
        &paths,
        Event::GameLoaded(first, 123, Err("offline".into())),
        decodes(),
    );
    assert_eq!(page.game_label(), "123");
    assert_eq!(page.looked_up, None);
    let _ = page.lookup_game(&paths);
    assert!(page.lookup_sequence > first);
}

#[test]
fn stale_game_lookup_cannot_replace_a_saved_match() {
    let (_root, paths, mut page) = fixture();
    page.game_id = Some(123);
    page.key_available = true;
    let _ = page.lookup_game(&paths);
    let old = page.lookup_sequence;
    page.lookup_sequence += 1;
    page.game = Some(Game {
        id: 123,
        name: "Picked".into(),
        year: None,
    });
    let _ = page.update(
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
    );
    assert_eq!(page.game_label(), "Picked");
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
    page.key_available = true;
    let game = Game {
        id: 77,
        name: "Celeste".into(),
        year: Some(2018),
    };
    let _ = page.update(&paths, Event::Change, decodes());
    page.change.as_mut().unwrap().selected = Some(Match(game.clone()));
    let _ = page.update(&paths, Event::SaveGame, decodes());
    assert!(page.busy());
    art::save_game(&paths, "game", game.id).unwrap();
    let step = page.update(
        &paths,
        Event::GameSaved(page.write_sequence, game, Ok(())),
        decodes(),
    );
    assert!(step.changed);
    assert!(!page.busy());
    assert_eq!(page.game_label(), "Celeste (2018)");
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
    art::set_manual(&paths, "game", None, Kind::Grid, &source).unwrap();
    snapshot(&mut page, &paths);

    assert!(page.has_override(Kind::Grid));
}

#[test]
fn late_lookup_and_preview_results_are_discarded_after_leaving() {
    let (root, paths, mut page) = fixture();
    page.game_id = Some(123);
    page.key_available = true;
    let _ = page.lookup_game(&paths);
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
    assert_eq!(page.game_label(), "123");

    let source = png(root.path());
    art::set_manual(&paths, "game", None, Kind::Grid, &source).unwrap();
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
