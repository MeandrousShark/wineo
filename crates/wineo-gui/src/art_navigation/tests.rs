use super::*;
use iced::event::Status;
use std::fs;
use wineo_core::art::{self, Kind};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::Environment;
use wineo_core::paths::Paths;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    for name in ["game", "other"] {
        Environment::create_native(&paths, &GlobalConfig::default(), name, None, false, |_| {})
            .unwrap();
    }
    let mut app = App::boot(paths.clone()).0;
    app.merge(crate::library::load_library(&paths).unwrap());
    (root, app)
}

#[test]
fn panel_and_both_context_menus_open_the_selected_environment_art() {
    for from_menu in [false, true] {
        for panel_open in [false, true] {
            let (_root, mut app) = fixture();
            app.selected = Some("other".into());
            app.panel_open = panel_open;
            if from_menu {
                let _ = app.update(Message::Context("game".into()));
                let _ = app.update(Message::ContextArt("game".into()));
            } else {
                app.selected = Some("game".into());
                let _ = app.update(Message::OpenArt);
            }
            assert_eq!(app.art_page.as_deref(), Some("game"));
            assert_eq!(app.selected.as_deref(), Some("game"));
            assert!(app.action_menu.is_none());
            assert!(app.settings.is_none());
            assert!(app.section_selected(sidebar::Section::Library));
            assert!(!app.section_selected(sidebar::Section::Settings));
        }
    }
}

#[test]
fn back_and_escape_return_to_the_library_and_preserve_the_panel() {
    for escape in [false, true] {
        let (_root, mut app) = fixture();
        app.selected = Some("game".into());
        app.panel_open = true;
        let _ = app.update(Message::OpenArt);
        let generation = app.art_pages["game"].generation;
        if escape {
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                Status::Ignored,
            ));
        } else {
            let _ = app.update(Message::ArtPage(
                "game".into(),
                generation,
                art_page::Event::Back,
            ));
        }
        assert!(app.art_page.is_none());
        assert!(app.panel_open);
        assert_eq!(app.selected.as_deref(), Some("game"));
        assert_eq!(app.section, sidebar::Section::Library);
        assert!(app.art_pages.contains_key("game"));
    }
}

#[test]
fn mouse_back_returns_from_art_like_escape() {
    let (_root, mut app) = fixture();
    app.selected = Some("game".into());
    app.panel_open = true;
    let _ = app.update(Message::OpenArt);
    assert!(app.art_page.is_some());
    crate::tests::press_back(&mut app, Status::Captured);
    assert!(app.art_page.is_none());
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("game"));
}

#[test]
fn sidebar_navigation_leaves_art_and_opens_its_destination() {
    for section in [
        sidebar::Section::Library,
        sidebar::Section::Trash,
        sidebar::Section::Checkup,
        sidebar::Section::Settings,
    ] {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::ContextArt("game".into()));
        let _ = app.update(Message::Section(section));
        assert!(app.art_page.is_none());
        assert!(app.section_selected(section));
        assert!(app.art_pages.contains_key("game"));
    }
}

#[test]
fn settings_hint_opens_global_settings_at_the_steamgriddb_key() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Settings,
    ));
    assert!(app.art_page.is_none());
    assert!(app.global_settings_open());
    assert_eq!(
        app.settings.as_ref().unwrap().reveal,
        Some(crate::settings::Reveal::SteamGridDbKey)
    );
}

#[test]
fn stale_navigation_cannot_close_another_environment_art_page() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::ContextArt("other".into()));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Back,
    ));
    assert_eq!(app.art_page.as_deref(), Some("other"));
    let _ = app.update(Message::ArtPage(
        "other".into(),
        generation,
        art_page::Event::Back,
    ));
    assert_eq!(app.art_page.as_deref(), Some("other"));
}

#[test]
fn art_page_does_not_accept_hidden_library_actions_or_launch_add() {
    let (_root, mut app) = fixture();
    app.search = "original".into();
    let _ = app.update(Message::ContextArt("game".into()));
    let _ = app.update(Message::SearchChanged("hidden".into()));
    let _ = app.update(Message::Select("other".into()));
    let _ = app.update(Message::Add);
    let _ = app.update(Message::CloneEnvironment("game".into()));
    let _ = app.update(Message::Play);
    assert_eq!(app.search, "original");
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.flow.is_none());
    assert!(app.operation.is_none());
}

fn browse(app: &mut App, path: std::path::PathBuf) -> (String, u64) {
    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Browse(wineo_core::art::Kind::Grid),
    ));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Picked(1, (None, wineo_core::art::Kind::Grid), Ok(Some(path))),
    ));
    assert!(app.art_busy());
    ("game".into(), generation)
}

#[test]
fn browse_completion_refreshes_the_card_and_evicts_old_art_cache() {
    use wineo_core::art::{self, Kind};
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    art::set_manual(
        &app.paths,
        "game",
        None,
        Kind::Grid,
        &source,
        wineo_core::test_support::identity(&app.paths, "game"),
    )
    .unwrap();
    app.merge(crate::library::load_library(&app.paths).unwrap());
    let old = app
        .cards
        .iter()
        .find(|card| card.name == "game")
        .unwrap()
        .art
        .clone()
        .unwrap();
    let old_key = (old.clone(), crate::artwork::Kind::Grid);
    app.images.insert(
        old_key.clone(),
        false,
        iced::widget::image::Handle::from_rgba(1, 1, vec![0; 4]),
    );
    ::image::RgbImage::new(10, 15).save(&source).unwrap();
    let (name, generation) = browse(&mut app, source.clone());
    let previous = app.library_generation;
    let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
    let _ = app.update(Message::ArtPage(
        name,
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(!app.art_busy());
    assert_ne!(app.library_generation, previous);
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::LibraryReloaded(app.library_generation, Ok(cards)));
    let new = app
        .cards
        .iter()
        .find(|card| card.name == "game")
        .unwrap()
        .art
        .clone()
        .unwrap();
    assert_ne!(new.identity, old.identity);
    assert!(!app.images.contains(&old_key, false));
    assert!(
        app.loading
            .contains(&((new, crate::artwork::Kind::Grid), false))
    );
}

#[test]
fn closing_after_leaving_art_waits_for_the_write_to_complete() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let (name, generation) = browse(&mut app, source.clone());
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    assert!(app.art_page.is_none());
    assert!(app.foreground_busy());
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.close_prompt && !app.closing);
    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && app.foreground_busy());
    let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
    let _ = app.update(Message::ArtPage(
        name,
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(!app.foreground_busy());
    assert!(app.paths.art_dir("game").join("grid.png").exists());
}

#[test]
fn refresh_closes_and_drops_art_after_external_removal() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    fs::remove_file(app.paths.environment_config("game")).unwrap();

    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));

    assert!(app.art_page.is_none());
    assert!(!app.art_pages.contains_key("game"));
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
}

#[test]
fn an_unreadable_library_keeps_the_art_page_open() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    let cards = crate::library::load_library(&app.paths).unwrap();
    let generation = app.art_pages["game"].generation;
    fs::write(app.paths.library_file(), "[broken\n").unwrap();

    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(app.art_page.as_deref(), Some("game"));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Back,
    ));
    assert!(app.art_pages.contains_key("game"));

    // Reopening looks at the library itself, not the page's cache.
    let _ = app.update(Message::ContextArt("game".into()));
    assert_eq!(app.art_page.as_deref(), Some("game"));
    assert!(app.art_pages.contains_key("game"));
}

#[test]
fn refresh_closes_art_after_external_rename() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    fs::rename(
        app.paths.environment_config("game"),
        app.paths.environment_config("renamed"),
    )
    .unwrap();

    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));

    assert!(app.art_page.is_none());
    assert!(!app.art_pages.contains_key("game"));
    assert!(app.selected.is_none());
    assert!(!app.panel_open);
}

#[test]
fn same_name_recreation_gets_a_fresh_art_page() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    let old_generation = app.art_pages["game"].generation;
    let old_identity = app.art_pages["game"].identity;
    wineo_core::test_support::replace_environment(&app.paths, "game").unwrap();

    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert!(app.art_page.is_none());
    assert!(!app.art_pages.contains_key("game"));

    let _ = app.update(Message::ContextArt("game".into()));

    let page = &app.art_pages["game"];
    assert_ne!(page.generation, old_generation);
    assert_ne!(page.identity, old_identity);
    assert_eq!(page.game_label(app.steamgriddb_available), "Not picked");
}

#[test]
fn refresh_reloads_open_art_after_an_external_art_change() {
    let (root, mut app) = fixture();
    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(10, 15).save(&source).unwrap();
    art::set_manual(
        &app.paths,
        "game",
        None,
        Kind::Grid,
        &source,
        wineo_core::test_support::identity(&app.paths, "game"),
    )
    .unwrap();
    fs::write(
        app.paths.environment_config("game"),
        "schema_version = 1\nkind = 'native'\n[targets.fresh]\nexe = 'notepad'\n",
    )
    .unwrap();
    let load_sequence = app.art_pages["game"].load_sequence;

    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));

    let page = &app.art_pages["game"];
    assert!(page.loading);
    assert!(page.load_sequence > load_sequence);
    let sequence = page.load_sequence;
    let snapshot = art_page::load_snapshot(&app.paths, "game").unwrap();
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Loaded(sequence, Ok(snapshot)),
    ));
    assert!(app.art_pages["game"].has_override(Kind::Grid));
    assert!(app.art_pages["game"].has_target("fresh"));
}

#[test]
fn late_write_result_after_close_does_not_update_cached_page() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let (name, generation) = browse(&mut app, source.clone());
    let _ = app.update(Message::ArtPage(
        name.clone(),
        generation,
        art_page::Event::Back,
    ));
    assert!(
        app.pending_art_writes
            .iter()
            .any(|(pending_name, _, _, _)| pending_name == &name)
    );

    let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
    let _ = app.update(Message::ArtPage(
        name,
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));

    assert!(!app.art_busy());
    assert!(!app.art_pages["game"].has_override(Kind::Grid));
}

#[test]
fn late_lookup_and_preview_results_after_close_are_dropped() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(10, 15).save(&source).unwrap();
    art::set_manual(
        &app.paths,
        "game",
        None,
        Kind::Grid,
        &source,
        wineo_core::test_support::identity(&app.paths, "game"),
    )
    .unwrap();
    art::save_game(&app.paths, "game", 77).unwrap();

    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let load_sequence = app.art_pages["game"].load_sequence;
    let snapshot = art_page::load_snapshot(&app.paths, "game").unwrap();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Loaded(load_sequence, Ok(snapshot)),
    ));
    let lookup_sequence = app.art_pages["game"].lookup_sequence;
    let preview_sequence = app.art_pages["game"].load_sequence;

    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Back,
    ));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::GameLoaded(
            lookup_sequence,
            77,
            Ok(Some(wineo_core::steamgriddb::Game {
                id: 77,
                name: "Stale game name".into(),
                year: None,
            })),
        ),
    ));
    let _ = app.update(Message::ArtPage(
        "game".into(),
        generation,
        art_page::Event::Previewed(
            preview_sequence,
            (None, Kind::Grid),
            Ok((
                iced::widget::image::Handle::from_rgba(1, 1, vec![0; 4]),
                Some((10, 15)),
            )),
        ),
    ));

    assert_eq!(
        app.art_pages["game"].game_label(app.steamgriddb_available),
        "77"
    );
    assert!(!app.art_pages["game"].has_preview(&(None, Kind::Grid)));
}

#[test]
fn a_failed_game_lookup_retries_with_backoff_until_it_succeeds_or_closes() {
    use std::time::Duration;
    let (_root, mut app) = fixture();
    art::save_game(&app.paths, "game", 77).unwrap();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    let _ = app.update(Message::ContextArt("game".into()));
    let generation = app.art_pages["game"].generation;
    let load_sequence = app.art_pages["game"].load_sequence;
    let snapshot = art_page::load_snapshot(&app.paths, "game");
    let send = |app: &mut App, event| {
        let _ = app.update(Message::ArtPage("game".into(), generation, event));
    };
    send(&mut app, art_page::Event::Loaded(load_sequence, snapshot));
    let mut expected = Duration::ZERO;
    for _ in 0..7 {
        let sequence = app.art_pages["game"].lookup_sequence;
        send(
            &mut app,
            art_page::Event::GameLoaded(sequence, 77, Err("offline".into())),
        );
        expected = art_page::next_backoff(expected);
        assert_eq!(app.art_pages["game"].lookup_backoff(), expected);
        send(
            &mut app,
            art_page::Event::RetryLookup(sequence.wrapping_sub(1), 77),
        );
        assert_eq!(
            app.art_pages["game"].lookup_sequence, sequence,
            "a stale retry is ignored"
        );
        send(&mut app, art_page::Event::RetryLookup(sequence, 77));
        assert_eq!(
            app.art_pages["game"].lookup_sequence,
            sequence + 1,
            "it looks up again"
        );
    }
    assert_eq!(expected, Duration::from_secs(64), "the wait stops growing");
    let sequence = app.art_pages["game"].lookup_sequence;
    send(
        &mut app,
        art_page::Event::GameLoaded(
            sequence,
            77,
            Ok(Some(wineo_core::steamgriddb::Game {
                id: 77,
                name: "Found".into(),
                year: None,
            })),
        ),
    );
    assert_eq!(app.art_pages["game"].lookup_backoff(), Duration::ZERO);
    assert_eq!(app.art_pages["game"].game_label(true), "Found");

    // A retry due after the page closed is dropped.
    let _ = app.update(Message::ContextArt("other".into()));
    let page = &app.art_pages["game"];
    let sequence = page.lookup_sequence;
    send(&mut app, art_page::Event::RetryLookup(sequence, 77));
    assert_eq!(app.art_pages["game"].lookup_sequence, sequence);
}

fn start_picker_use(app: &mut App) -> (u64, u64, wineo_core::cancel::Token) {
    start_picker_use_for(app, "game")
}

fn start_picker_use_for(app: &mut App, name: &str) -> (u64, u64, wineo_core::cancel::Token) {
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    let _ = app.open_art(name.into());
    let generation = app.art_pages[name].generation;
    art::save_game(&app.paths, name, 9).unwrap();
    let snapshot = art_page::load_snapshot(&app.paths, name).unwrap();
    let sequence = app.art_pages[name].load_sequence;
    let _ = app.art_update(
        name.into(),
        generation,
        art_page::Event::Loaded(sequence, Ok(snapshot)),
    );
    let _ = app.art_update(name.into(), generation, art_page::Event::Find(Kind::Grid));
    let event = app.art_pages[name]
        .picker_event(art_picker::Event::Listed(
            1,
            Ok(vec![wineo_core::steamgriddb::Image {
                id: 914,
                url: "https://cdn.example/chosen.png".into(),
                width: Some(600),
                height: Some(900),
                ..Default::default()
            }]),
        ))
        .unwrap();
    let visit = match &event {
        art_page::Event::Picker(visit, _) => *visit,
        _ => unreachable!(),
    };
    let _ = app.art_update(name.into(), generation, event);
    let event = app.art_pages[name]
        .picker_event(art_picker::Event::Select(0))
        .unwrap();
    let _ = app.art_update(name.into(), generation, event);
    let _ = app.art_key(&Key::Named(Named::Enter), Modifiers::empty(), false);
    assert!(app.art_busy());
    let token = app.pending_art_cancels.values().next().unwrap().clone();
    (generation, visit, token)
}

#[test]
fn picker_escape_returns_to_art_before_leaving_art_and_use_keeps_running() {
    let (_root, mut app) = fixture();
    let (_, _, token) = start_picker_use(&mut app);
    let _ = app.art_key(&Key::Named(Named::Escape), Modifiers::empty(), false);
    assert_eq!(app.art_page.as_deref(), Some("game"));
    assert!(!app.art_pages["game"].has_overlay());
    assert!(app.art_busy());
    assert!(!token.is_canceled());
    let _ = app.art_key(&Key::Named(Named::Escape), Modifiers::empty(), false);
    assert!(app.art_page.is_none());
    assert!(app.art_busy());
    assert!(!token.is_canceled());
}

#[test]
fn picker_use_completion_after_reopening_refreshes_card_and_active_art() {
    let (root, mut app) = fixture();
    let (generation, visit, token) = start_picker_use(&mut app);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    let _ = app.open_art("game".into());
    let reopened = app.art_pages["game"].generation;
    assert_ne!(generation, reopened);
    let _ = app.art_update(
        "game".into(),
        generation,
        art_page::Event::UseProgress(1, 7, Some(20)),
    );
    assert!(!token.is_canceled());
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let outcome = art_page::write_art(&app.paths, "game", &(None, Kind::Grid), Some(&source));
    let previous = app.library_generation;
    let _ = app.art_update(
        "game".into(),
        generation,
        art_page::Event::Used(1, visit, (None, Kind::Grid), outcome),
    );
    assert!(!app.art_busy());
    assert!(app.pending_art_cancels.is_empty());
    assert_ne!(app.library_generation, previous);
    assert_eq!(app.art_pages["game"].generation, reopened);
    assert!(app.art_pages["game"].loading);
}

#[test]
fn closing_during_picker_use_cancels_and_waits_even_after_leaving() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    art::set_manual(
        &app.paths,
        "game",
        None,
        Kind::Grid,
        &source,
        wineo_core::test_support::identity(&app.paths, "game"),
    )
    .unwrap();
    let before = fs::read(app.paths.art_dir("game").join("grid.png")).unwrap();
    let (generation, visit, token) = start_picker_use(&mut app);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.close_prompt && !app.closing);
    let _ = app.update(Message::CancelAndClose);
    assert!(token.is_canceled());
    assert!(app.closing && app.foreground_busy());
    let outcome = art_page::canceled_use_outcome();
    let _ = app.art_update(
        "game".into(),
        generation,
        art_page::Event::Used(1, visit, (None, Kind::Grid), outcome),
    );
    assert!(!app.foreground_busy());
    assert!(app.pending_art_cancels.is_empty());
    assert_eq!(
        fs::read(app.paths.art_dir("game").join("grid.png")).unwrap(),
        before
    );
}

/// Runs `task` to its end and returns the messages it produced.
fn messages(task: Task<Message>) -> Vec<Message> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut messages = Vec::new();
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
        {
            if let iced_runtime::Action::Output(message) = action {
                messages.push(message);
            }
        }
        messages
    })
}

fn titled(app: &App, name: &str, title: &str) -> Vec<crate::library::Card> {
    let mut cards = crate::library::load_library(&app.paths).unwrap();
    for card in &mut cards {
        if card.name == name {
            card.display_title = title.to_owned();
        }
    }
    cards
}

fn change_search(app: &mut App, name: &str) -> Option<String> {
    let generation = app.art_pages[name].generation;
    let _ = app.art_update(name.to_owned(), generation, art_page::Event::Change);
    app.art_pages[name].change_search().map(str::to_owned)
}

#[test]
fn choose_artwork_on_finished_opens_the_art_page_once_the_library_loads() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    app.flow_generation = 1;
    app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
    let _ = app.flow_update(1, crate::flow::Event::New);
    let action = crate::flow::Event::Create(Box::new(crate::flow::new::Event::ChooseArt));
    // A New page that isn't on Finished doesn't leave.
    let _ = app.flow_update(1, action.clone());
    assert!(app.flow.is_some());

    let page = crate::flow::new::Page::finished_for_tests(app.paths.clone(), "game");
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    let reload = app.flow_update(app.flow_generation, action);
    assert!(app.flow.is_none());
    assert!(app.art_page.is_none(), "the library loads first");
    for message in messages(reload) {
        let _ = app.update(message);
    }
    assert_eq!(app.art_page.as_deref(), Some("game"));
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open);
    assert!(app.operation.is_none(), "it never launches");
    assert_eq!(app.art_pages["game"].title, "game");
}

#[test]
fn choose_artwork_opens_the_art_page_with_the_card_title() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    let cards = titled(&app, "game", "Assassin's Creed: Origins");
    let _ = app.flow_returned(
        crate::flow::Action::Art {
            name: "game".into(),
        },
        Ok(cards),
    );
    assert_eq!(app.art_page.as_deref(), Some("game"));
    assert_eq!(
        change_search(&mut app, "game").as_deref(),
        Some("Assassin's Creed: Origins")
    );
}

#[test]
fn choose_artwork_while_closing_opens_nothing() {
    let (_root, mut app) = fixture();
    app.closing = true;
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.flow_returned(
        crate::flow::Action::Art {
            name: "game".into(),
        },
        Ok(cards),
    );
    assert!(app.art_page.is_none());
    assert!(app.selected.is_none());
}

#[test]
fn choose_artwork_refused_leaves_the_card_selected_with_its_panel() {
    let (_root, mut app) = fixture();
    // The close prompt opened while the library loaded.
    app.close_prompt = true;
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.flow_returned(
        crate::flow::Action::Art {
            name: "game".into(),
        },
        Ok(cards),
    );
    assert!(app.art_page.is_none());
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open);
}

#[test]
fn the_art_page_searches_for_what_the_card_shows() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    app.merge(titled(&app, "game", "Notepad++"));
    app.selected = Some("game".into());
    let _ = app.update(Message::OpenArt);
    assert_eq!(
        change_search(&mut app, "game").as_deref(),
        Some("Notepad++")
    );
    let generation = app.art_pages["game"].generation;
    let _ = app.art_update("game".into(), generation, art_page::Event::CancelChange);
    let _ = app.art_update("game".into(), generation, art_page::Event::Back);
    assert!(app.art_pages.contains_key("game"), "the page stays cached");

    // The cached page takes the title the card shows now.
    app.merge(titled(&app, "game", "Notepad Plus"));
    let _ = app.update(Message::OpenArt);
    assert_eq!(
        change_search(&mut app, "game").as_deref(),
        Some("Notepad Plus")
    );

    // A blank title falls back to the environment name.
    let generation = app.art_pages["game"].generation;
    let _ = app.art_update("game".into(), generation, art_page::Event::CancelChange);
    let _ = app.art_update("game".into(), generation, art_page::Event::Back);
    app.merge(titled(&app, "game", "  "));
    let _ = app.update(Message::OpenArt);
    assert_eq!(change_search(&mut app, "game").as_deref(), Some("game"));
}

#[test]
fn a_cached_page_holding_a_fingerprint_is_reopened_after_another_process_mints() {
    let (_root, mut app) = fixture();
    let config = app.paths.environment_config("legacy");
    fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = app.update(Message::ContextArt("legacy".into()));
    let page = &app.art_pages["legacy"];
    let (generation, identity) = (page.generation, page.identity);
    assert_eq!(identity.id(), None);
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Back,
    ));
    assert!(app.art_page.is_none());
    // Leaving moves the cached page to a new generation, which a page made
    // anew wouldn't have.
    let generation = app.art_pages["legacy"].generation;

    let minted = art_page::EnvironmentIdentity::acquire(&app.paths, "legacy")
        .unwrap()
        .unwrap();
    assert!(minted.id().is_some());
    let _ = app.update(Message::ContextArt("legacy".into()));

    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    let page = &app.art_pages["legacy"];
    assert_eq!(page.generation, generation);
    assert_eq!(page.identity, identity);
}

/// Rewrites `name`'s config through a new file, as an editor saving it
/// would.
fn edit_config(paths: &Paths, name: &str) {
    let config = paths.environment_config(name);
    let text = fs::read_to_string(&config).unwrap();
    let staged = config.with_extension("saving");
    fs::write(&staged, format!("# edited\n{text}")).unwrap();
    fs::rename(staged, config).unwrap();
}

/// Opens the Art page of `legacy`, an environment from before ids, and
/// starts a grid write from `source` on it.
fn legacy_write(app: &mut App, source: &std::path::Path) -> u64 {
    let config = app.paths.environment_config("legacy");
    fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
    let _ = app.update(Message::ContextArt("legacy".into()));
    let generation = app.art_pages["legacy"].generation;
    assert_eq!(app.art_pages["legacy"].identity.id(), None);
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Browse(Kind::Grid),
    ));
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Picked(1, (None, Kind::Grid), Ok(Some(source.to_owned()))),
    ));
    assert!(app.art_busy());
    generation
}

#[test]
fn an_old_environments_page_adopts_the_id_from_its_first_write_and_then_survives_edits() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    assert!(id.id().is_some());
    // Edited before the result lands, which the fingerprint wouldn't survive.
    edit_config(&app.paths, "legacy");
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert_eq!(app.art_pages["legacy"].identity, id);
    assert!(app.pending_art_writes.is_empty());
    assert!(!app.art_busy());
    assert!(app.paths.art_dir("legacy").join("grid.png").exists());

    // Another edit keeps it open; a real replacement closes it.
    edit_config(&app.paths, "legacy");
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    wineo_core::test_support::replace_environment(&app.paths, "legacy").unwrap();
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
    assert!(app.art_page.is_none());
    assert!(!app.art_pages.contains_key("legacy"));
}

#[test]
fn a_page_left_during_its_first_write_still_adopts_the_id() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    assert!(app.art_page.is_none());
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(!app.art_busy());
    assert_eq!(app.art_pages["legacy"].identity, id);
    // So reopening after an edit keeps the cached page.
    let cached = app.art_pages["legacy"].generation;
    edit_config(&app.paths, "legacy");
    let _ = app.update(Message::ContextArt("legacy".into()));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert_eq!(app.art_pages["legacy"].generation, cached);
}

/// Adds `legacy`, an environment from before ids, to the library.
fn add_legacy(app: &mut App) {
    let config = app.paths.environment_config("legacy");
    fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
}

fn rescan(app: &mut App) {
    let cards = crate::library::load_library(&app.paths).unwrap();
    let _ = app.update(Message::Loaded(Ok(cards)));
}

#[test]
fn an_edit_during_an_old_environments_first_write_closes_nothing_before_its_result() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    // The worker has upgraded and written when the edit comes.
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    edit_config(&app.paths, "legacy");
    rescan(&mut app);
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Showing(art_page::Showing(None)),
    ));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert!(app.art_pages["legacy"].busy());
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert_eq!(app.art_pages["legacy"].identity, id);
}

#[test]
fn reopening_mid_first_write_after_an_edit_keeps_the_writing_page() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    edit_config(&app.paths, "legacy");
    let _ = app.update(Message::ContextArt("legacy".into()));
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    let page = &app.art_pages["legacy"];
    assert!(page.busy(), "the cached page keeps its write");
    let reopened = page.generation;
    // A second write can't start beside the first.
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        reopened,
        art_page::Event::Remove(Kind::Grid),
    ));
    assert_eq!(app.pending_art_writes.len(), 1);
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(!app.art_busy());
    assert_eq!(app.art_pages["legacy"].identity, id);
}

#[test]
fn a_replacement_during_the_first_write_closes_the_page_when_its_result_comes() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    let held = app.art_pages["legacy"].identity;
    wineo_core::test_support::replace_environment(&app.paths, "legacy").unwrap();
    rescan(&mut app);
    assert_eq!(app.art_page.as_deref(), Some("legacy"), "the write decides");
    let outcome = art_page::write_art_with(
        &app.paths,
        "legacy",
        held,
        &(None, Kind::Grid),
        Some(&source),
    );
    assert_eq!(outcome.identity(), None);
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(app.art_page.is_none());
    assert!(!app.art_pages.contains_key("legacy"));
    assert!(!app.art_busy());
}

#[test]
fn a_page_holding_an_id_still_closes_at_once_on_a_replacement() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let _ = browse(&mut app, source);
    assert!(app.art_pages["game"].identity.id().is_some());
    wineo_core::test_support::replace_environment(&app.paths, "game").unwrap();
    rescan(&mut app);
    assert!(app.art_page.is_none());
}

#[test]
fn a_replacement_just_before_a_stored_result_still_reloads_the_library() {
    let (root, mut app) = fixture();
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let generation = legacy_write(&mut app, &source);
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    assert!(outcome.changed());
    wineo_core::test_support::replace_environment(&app.paths, "legacy").unwrap();
    let previous = app.library_generation;
    let _ = app.update(Message::ArtPage(
        "legacy".into(),
        generation,
        art_page::Event::Wrote(1, (None, Kind::Grid), outcome),
    ));
    assert!(app.art_page.is_none());
    assert!(!app.art_busy());
    assert_ne!(app.library_generation, previous, "the stored art shows");
}

#[test]
fn a_use_left_mid_download_clears_its_keys_and_adopts_the_id() {
    let (root, mut app) = fixture();
    add_legacy(&mut app);
    let (generation, visit, _) = start_picker_use_for(&mut app, "legacy");
    assert_eq!(app.art_pages["legacy"].identity.id(), None);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    edit_config(&app.paths, "legacy");
    let _ = app.art_update(
        "legacy".into(),
        generation,
        art_page::Event::Used(1, visit, (None, Kind::Grid), outcome),
    );
    assert!(
        app.pending_art_writes.is_empty(),
        "{:?}",
        app.pending_art_writes
    );
    assert!(app.pending_art_cancels.is_empty());
    assert_eq!(app.art_pages["legacy"].identity, id);
}

#[test]
fn a_use_on_the_page_adopts_the_id_before_an_edit_can_close_it() {
    let (root, mut app) = fixture();
    add_legacy(&mut app);
    let (generation, visit, _) = start_picker_use_for(&mut app, "legacy");
    let source = root.path().join("grid.png");
    ::image::RgbImage::new(2, 3).save(&source).unwrap();
    let outcome = art_page::write_art(&app.paths, "legacy", &(None, Kind::Grid), Some(&source));
    let id = outcome.identity().unwrap();
    edit_config(&app.paths, "legacy");
    let _ = app.art_update(
        "legacy".into(),
        generation,
        art_page::Event::Used(1, visit, (None, Kind::Grid), outcome),
    );
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert_eq!(app.art_pages["legacy"].identity, id);
    assert!(app.pending_art_writes.is_empty());
}

#[test]
fn a_saved_game_adopts_the_id_it_upgraded_to() {
    let (_root, mut app) = fixture();
    add_legacy(&mut app);
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, true));
    let _ = app.update(Message::ContextArt("legacy".into()));
    let generation = app.art_pages["legacy"].generation;
    let game = wineo_core::steamgriddb::Game {
        id: 77,
        name: "Legacy".into(),
        year: None,
    };
    let _ = app.art_update("legacy".into(), generation, art_page::Event::Change);
    let _ = app.art_update(
        "legacy".into(),
        generation,
        art_page::Event::PickGame(crate::flow::artwork::Match(game.clone())),
    );
    let _ = app.art_update("legacy".into(), generation, art_page::Event::SaveGame);
    assert!(app.art_busy());
    let sequence = app.art_pages["legacy"].write_sequence;
    // What the save's task does: upgrade, then save under the id.
    let id = app.art_pages["legacy"]
        .identity
        .upgrade(&app.paths, "legacy")
        .unwrap();
    assert!(id.id().is_some());
    art::save_game_for(&app.paths, "legacy", 77, id, None).unwrap();
    edit_config(&app.paths, "legacy");
    let _ = app.art_update(
        "legacy".into(),
        generation,
        art_page::Event::GameSaved(sequence, game, Ok(()), Some(id)),
    );
    assert_eq!(app.art_page.as_deref(), Some("legacy"));
    assert_eq!(app.art_pages["legacy"].identity, id);
    assert!(!app.art_busy());
}
