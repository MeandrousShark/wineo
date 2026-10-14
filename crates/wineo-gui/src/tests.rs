use super::*;
use std::time::SystemTime;

#[test]
fn app_settings_carry_the_bundled_fonts() {
    let settings = app_settings();

    assert_eq!(settings.default_text_size, 14.into());
    assert_eq!(settings.default_font, fonts::DEFAULT);
    assert_eq!(settings.fonts.len(), 2);
    assert_eq!(settings.fonts[0].as_ref(), fonts::REGULAR);
    assert_eq!(settings.fonts[1].as_ref(), fonts::SEMIBOLD);
}

pub(crate) fn card(name: &str, state: State, seconds: i64) -> Card {
    Card {
        name: name.into(),
        kind: None,
        display_title: name.into(),
        steam: None,
        state,
        lost: false,
        prefix: None,
        busy: false,
        busy_holder: None,
        unresolved: false,
        error: None,
        playtime: playtime_label(seconds),
        sessions: "0".to_owned(),
        last_played: "never".into(),
        tags: Vec::new(),
        targets: vec!["game".into()],
        default_target: Some("game".into()),
        art: None,
        panel_art: HashMap::new(),
        gog: None,
    }
}
pub(crate) fn app() -> App {
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| "/tmp/wineo-gui-tests".into())).unwrap();
    App::boot(paths).0
}

fn tool_app() -> App {
    let mut app = app();
    app.cards = ["notes", "alias", "editor"]
        .into_iter()
        .map(|name| {
            let mut card = card(name, State::Inactive, 0);
            card.prefix = Some(PathBuf::from(if name == "editor" {
                "/tmp/editor-prefix"
            } else {
                "/tmp/notes-prefix"
            }));
            card
        })
        .collect();
    let _ = app.update(Message::OpenTool(
        "notes".into(),
        tools_page::Kind::Winetricks,
    ));
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::OpenWinetricks,
    ));
    app
}

/// Snapshots, Run program, and Winetricks are Wine-only. Clone and Copy
/// to portable work for native environments too, but never for a proxied
/// one, whose launcher owns it.
#[test]
fn the_context_menu_follows_each_card_kind() {
    use crate::library::CardKind;

    let mut app = app();
    app.cards = [
        ("notes", Some(CardKind::Wine), true),
        ("dusk", Some(CardKind::Native), false),
        ("bg3", Some(CardKind::Proxied), false),
        ("broken", None, false),
    ]
    .into_iter()
    .map(|(name, kind, prefix)| {
        let mut card = card(name, State::Inactive, 0);
        card.kind = kind;
        card.prefix = prefix.then(|| PathBuf::from("/tmp/notes-prefix"));
        card
    })
    .collect();
    for (name, wine, transfer) in [
        ("notes", true, true),
        ("dusk", false, true),
        ("bg3", false, false),
        ("broken", false, true),
    ] {
        assert_eq!(app.wine_actions(name), wine, "{name}");
        assert_eq!(app.transfer_actions(name), transfer, "{name}");
    }

    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::CloneEnvironment("bg3".into()));
    assert!(app.flow.is_none());
    let _ = app.update(Message::CopyPortable("bg3".into()));
    assert!(app.portable_page.is_none());
    let _ = app.update(Message::OpenTool("bg3".into(), tools_page::Kind::Exec));
    assert!(app.tool_page.is_none());
    let _ = app.update(Message::OpenTool(
        "bg3".into(),
        tools_page::Kind::Winetricks,
    ));
    assert!(app.tool_page.is_none());
    let _ = app.update(Message::SnapshotsEnvironment("bg3".into()));
    assert!(app.snapshots_page.is_none());

    let _ = app.update(Message::CopyPortable("dusk".into()));
    assert_eq!(app.portable_page.as_deref(), Some("dusk"));
}

#[test]
fn native_cards_can_still_be_cloned() {
    use crate::library::CardKind;

    let mut app = app();
    let mut native = card("dusk", State::Inactive, 0);
    native.kind = Some(CardKind::Native);
    app.cards = vec![native];
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::CloneEnvironment("dusk".into()));
    assert!(app.flow.is_some());
}

#[test]
fn tools_block_shared_prefix_changes_but_leave_other_environments_usable() {
    let mut app = tool_app();
    assert!(app.tools_busy());
    assert!(!app.management_busy());
    assert!(app.remove_reason("notes").is_some());
    assert!(app.remove_reason("alias").is_some());
    assert!(app.remove_reason("editor").is_none());
    assert!(app.tool_blocks_launch("alias"));
    assert!(!app.tool_blocks_launch("editor"));
    app.cards[0].prefix = Some("/tmp/replaced-prefix".into());
    assert!(app.tool_prefix_busy("alias"));
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::CloneEnvironment("editor".into()));
    assert!(app.flow.is_some());
    assert!(app.tools_busy());
}

#[test]
fn hidden_tool_messages_keep_worker_but_cannot_navigate_or_start_work() {
    let mut app = tool_app();
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::Settings,
    ));
    assert!(app.settings.is_none());
    assert!(app.tools_busy());
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::Finished(999, Ok(Some(0))),
    ));
    assert!(app.tools_busy());
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::Finished(1, Ok(Some(0))),
    ));
    assert!(!app.tools_busy());
    assert!(app.tool_page.is_none());
    let _ = app.update(Message::OpenTool(
        "notes".into(),
        tools_page::Kind::Winetricks,
    ));
    assert!(!app.tools_busy());
    assert_eq!(app.tool_pages.len(), 1);
}

#[test]
fn closing_waits_for_all_tools_after_leaving_their_pages() {
    let mut app = tool_app();
    let _ = app.update(Message::OpenTool(
        "editor".into(),
        tools_page::Kind::Winetricks,
    ));
    let _ = app.update(Message::Tool(
        "editor".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::OpenWinetricks,
    ));
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    let _ = app.update(Message::Window(window::Event::CloseRequested));
    assert!(app.close_prompt);
    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && app.foreground_busy());
    assert!(
        app.tool_pages
            .values()
            .all(tools_page::Page::force_available)
    );
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::Finished(1, Ok(Some(0))),
    ));
    assert!(app.foreground_busy());
    let _ = app.update(Message::Tool(
        "editor".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::Finished(1, Ok(Some(0))),
    ));
    assert!(!app.foreground_busy());
}

#[test]
fn portable_review_uses_the_slot_and_survives_sidebar_navigation() {
    let mut app = app();
    let _ = app.update(Message::CopyPortable("native".into()));
    let _ = app.update(Message::Portable(
        "native".into(),
        portable_page::Event::SourceLoaded(Ok(true)),
    ));
    let _ = app.update(Message::Portable(
        "native".into(),
        portable_page::Event::Folder("/tmp/portable-review".into()),
    ));
    let _ = app.update(Message::Portable(
        "native".into(),
        portable_page::Event::Review,
    ));
    assert!(app.filesystem_busy() && app.management_busy());
    let _ = app.update(Message::Section(sidebar::Section::Checkup));
    assert!(app.portable_page.is_none());
    assert!(app.filesystem_busy());
    let _ = app.update(Message::CopyPortable("second".into()));
    let _ = app.update(Message::Portable(
        "second".into(),
        portable_page::Event::SourceLoaded(Ok(true)),
    ));
    let _ = app.update(Message::Portable(
        "second".into(),
        portable_page::Event::Folder("/tmp/second-copy".into()),
    ));
    let _ = app.update(Message::Portable(
        "second".into(),
        portable_page::Event::Review,
    ));
    assert!(!app.portable_pages["second"].busy());
    assert!(app.portable_pages["native"].busy());
}

#[test]
fn snapshot_shortcut_preserves_downloads_shortcut_and_page_results() {
    assert_eq!(
        sidebar::shortcut(
            &keyboard::Key::Character("4".into()),
            keyboard::Modifiers::CTRL
        ),
        Some(sidebar::Section::Downloads)
    );
    assert_eq!(
        sidebar::shortcut(
            &keyboard::Key::Character("5".into()),
            keyboard::Modifiers::CTRL
        ),
        Some(sidebar::Section::Snapshots)
    );
    let mut app = app();
    let _ = app.update(Message::SnapshotsEnvironment("notes".into()));
    assert_eq!(app.section, sidebar::Section::Snapshots);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::Section(sidebar::Section::Snapshots));
    assert_eq!(
        app.snapshots_page.as_ref().unwrap().selected_name(),
        Some("notes")
    );
}

#[test]
fn cached_winetricks_page_refreshes_declarations_on_reopen() {
    let mut app = app();
    let mut item = card("notes", State::Inactive, 0);
    item.prefix = Some("/tmp/notes-prefix".into());
    app.cards.push(item);
    let _ = app.update(Message::OpenTool(
        "notes".into(),
        tools_page::Kind::Winetricks,
    ));
    let _ = app.update(Message::Section(sidebar::Section::Library));
    let task = app.update(Message::OpenTool(
        "notes".into(),
        tools_page::Kind::Winetricks,
    ));
    assert!(task.units() > 0);
    assert_eq!(app.tool_pages.len(), 1);
    assert!(!app.tools_busy());
}

#[test]
fn winetricks_refuses_running_shared_prefix_but_exec_can_coexist() {
    let mut app = app();
    app.cards = ["notes", "alias"]
        .into_iter()
        .map(|name| {
            let mut card = card(
                name,
                if name == "alias" {
                    State::Running
                } else {
                    State::Inactive
                },
                0,
            );
            card.prefix = Some("/tmp/shared-prefix".into());
            card
        })
        .collect();
    assert!(
        app.tool_start_reason("notes", tools_page::Kind::Winetricks)
            .is_some()
    );
    assert!(
        app.tool_start_reason("notes", tools_page::Kind::Exec)
            .is_none()
    );
    assert!(app.remove_reason("notes").is_some());
    let _ = app.update(Message::OpenTool(
        "notes".into(),
        tools_page::Kind::Winetricks,
    ));
    let _ = app.update(Message::Tool(
        "notes".into(),
        tools_page::Kind::Winetricks,
        tools_page::Event::OpenWinetricks,
    ));
    assert!(!app.tools_busy());
}

#[test]
fn restore_markers_block_ordinary_changes_without_disabling_recovery() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let marker = paths.restore_marker("notes");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(marker, "invalid marker").unwrap();
    let app = App::boot(paths).0;
    assert_eq!(
        app.remove_reason("notes").as_deref(),
        Some("Roll back restore first.")
    );
    assert!(
        app.tool_start_reason("notes", tools_page::Kind::Exec)
            .is_some()
    );
    assert!(
        app.tool_start_reason("notes", tools_page::Kind::Winetricks)
            .is_some()
    );
    assert!(app.prefix_change_reason("notes").is_none());
    let marker = app.paths.restore_marker("notes");
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(root.path().join("missing-marker"), marker).unwrap();
    assert!(app.restore_pending("notes"));
    assert!(app.remove_reason("notes").is_some());
    assert!(
        app.tool_start_reason("notes", tools_page::Kind::Exec)
            .is_some()
    );
    assert!(app.prefix_change_reason("notes").is_none());
}

#[test]
fn portable_success_notifies_destination_and_enter_returns_to_library() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    std::fs::create_dir_all(paths.environments_dir()).unwrap();
    std::fs::write(paths.environment_config("native"), "kind = 'native'\n").unwrap();
    let folder = root.path().join("portable");
    let destination = Paths::select(
        |key| (key == "HOME").then(|| root.path().as_os_str().to_owned()),
        Some(&folder),
    )
    .unwrap();
    let review = wineo_core::portable::inspect_native(&paths, &destination, "native").unwrap();
    let mut app = App::boot(paths).0;
    let _ = app.update(Message::CopyPortable("native".into()));
    for event in [
        portable_page::Event::SourceLoaded(Ok(true)),
        portable_page::Event::Folder(folder.display().to_string()),
        portable_page::Event::Review,
        portable_page::Event::ReviewLoaded(
            1,
            Ok(Box::new(portable_page::Review::Native(Box::new(review)))),
        ),
        portable_page::Event::Copy,
        portable_page::Event::CopyFinished(2, Ok(())),
    ] {
        let _ = app.update(Message::Portable("native".into(), event));
    }
    assert_eq!(
        app.feature_notice.as_ref().unwrap().0,
        format!("Copied native to {}.", folder.display())
    );
    let _ = app.feature_key(
        &keyboard::Key::Named(keyboard::key::Named::Enter),
        keyboard::Modifiers::empty(),
        false,
    );
    assert!(app.portable_page.is_none());
    assert!(!app.filesystem_busy());
}

#[test]
fn overlays_preserve_library_scroll_state() {
    use iced::advanced::widget::{Tree, tree};

    fn scroll_state(widget: &Tree, tag: tree::Tag) -> Option<*const ()> {
        if widget.tag == tag
            && let tree::State::Some(state) = &widget.state
        {
            return Some(std::ptr::from_ref(state.as_ref()).cast());
        }
        widget
            .children
            .iter()
            .find_map(|child| scroll_state(child, tag))
    }

    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let scroll: iced::Element<'_, Message> = scrollable(iced::widget::Space::new()).into();
    let tag = scroll.as_widget().tag();
    let mut tree = Tree::new(app.view().as_widget());
    // The offset lives in iced's private scrollable state, not App::scroll_offset.
    let state = scroll_state(&tree, tag).unwrap();

    let _ = app.update(Message::Context("game".into()));
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));
    let _ = app.update(Message::CloseMenu);
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));

    let _ = app.update(Message::Select("game".into()));
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));
    let _ = app.update(Message::ClosePanel);
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));

    app.flow = Some(flow::Page::choose(app.paths.clone()));
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));
    app.close_prompt = true;
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));
    app.flow = None;
    app.close_prompt = false;
    tree.diff(app.view().as_widget());
    assert_eq!(scroll_state(&tree, tag), Some(state));
}

#[test]
fn an_open_action_menu_stays_put_until_the_next_context() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let first = iced::Point::new(30.0, 40.0);
    let _ = app.update(Message::MenuAnchor(first));
    let _ = app.update(Message::Context("game".into()));
    assert_eq!(app.action_menu_position, first);
    // Releasing the menu's own scrollbar publishes a new anchor.
    let _ = app.update(Message::MenuAnchor(iced::Point::new(300.0, 400.0)));
    assert_eq!(app.action_menu.as_deref(), Some("game"));
    assert_eq!(app.action_menu_position, first);
}

#[test]
fn late_art_and_add_results_use_current_key_availability() {
    for (initial, available, deliver_results_after) in [
        (false, true, false),
        (false, true, true),
        (true, false, false),
        (true, false, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        let config = paths.environment_config("game");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();

        let mut app = App::boot(paths.clone()).0;
        let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(1, initial));
        let identity = art_page::EnvironmentIdentity::read(&paths, "game").unwrap();
        app.art_page = Some("game".to_owned());
        app.art_pages.insert(
            "game".to_owned(),
            art_page::Page::new("game".to_owned(), 7, identity),
        );
        let snapshot = art_page::load_snapshot(&paths, "game").unwrap();
        app.flow_generation = 1;
        app.flow = Some(flow::Page::choose(paths.clone()));
        let _ = app.flow_update(1, flow::Event::New);
        let configured = || {
            flow::Event::Create(Box::new(flow::new::Event::Art(
                flow::artwork::Event::Configured(1, Ok(wineo_core::config::ArtFetch::Icon)),
            )))
        };
        let loaded = || {
            Message::ArtPage(
                "game".to_owned(),
                7,
                art_page::Event::Loaded(1, Ok(snapshot.clone())),
            )
        };

        if !deliver_results_after {
            let _ = app.update(loaded());
            let _ = app.flow_update(1, configured());
        }
        app.steamgriddb_completed_sequence = 2;
        let _ = app.update(Message::SteamGridDbKeyChangedAt(2, available));
        if deliver_results_after {
            let _ = app.update(loaded());
            let _ = app.flow_update(1, configured());
        }

        assert_eq!(app.steamgriddb_available, available);
        let _ = app.art_update(
            "game".to_owned(),
            7,
            art_page::Event::Find(wineo_core::art::Kind::Grid),
        );
        assert_eq!(app.art_pages["game"].is_changing(), available);
        if available {
            let _ = app.art_update("game".to_owned(), 7, art_page::Event::CancelChange);
        }
        let _ = app.art_update("game".to_owned(), 7, art_page::Event::Change);
        assert_eq!(app.art_pages["game"].is_changing(), available);
        let _ = app.flow_update(
            1,
            flow::Event::Create(Box::new(flow::new::Event::Art(
                flow::artwork::Event::Fetch(true),
            ))),
        );
        assert_eq!(app.flow.as_ref().unwrap().art_fetching(), available);

        let _ = app.update(Message::SteamGridDbKeyChangedAt(1, !available));
        assert_eq!(app.steamgriddb_available, available);
    }
}

#[test]
fn steamgriddb_availability_never_regresses_to_an_older_sequence() {
    let mut app = app();
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(3, true));
    let _ = app.update(Message::SteamGridDbKeyPresenceLoaded(2, false));
    assert!(app.steamgriddb_available);
}
pub(crate) fn art_file(path: &str, len: u64) -> ArtFile {
    ArtFile {
        path: PathBuf::from(path),
        identity: ArtIdentity {
            modified: SystemTime::UNIX_EPOCH,
            len,
        },
    }
}
fn card_with_art(art: ArtFile) -> Card {
    let mut card = card("game", State::Inactive, 0);
    card.art = Some(art);
    card
}
#[test]
fn app_icon_renders_from_embedded_svg() {
    assert!(app_icon().is_some());
}

#[test]
fn an_empty_library_stays_shown_through_background_refreshes() {
    let mut app = app();
    assert!(!app.loaded, "nothing claims empty before the first load");
    let _ = app.update(Message::Loaded(Ok(Vec::new())));
    assert!(app.loaded);
    let _ = app.update(Message::Refresh);
    assert!(app.refreshing);
    assert!(app.loaded, "a refresh must not hide the empty state");
}

#[test]
fn back_from_new_returns_to_add_choices_without_closing() {
    let mut app = app();
    let _ = app.update(Message::Add);
    let generation = app.flow_generation;
    let _ = app.flow_update(generation, flow::Event::New);
    let _ = app.flow_update(generation, flow::Event::Back);
    assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
    assert!(!app.closing);
    assert_ne!(app.flow_generation, generation);
    let _ = app.flow_update(generation, flow::Event::Back);
    assert!(app.flow.is_some(), "stale Back cannot dismiss Add");
}

#[test]
fn creation_overlay_rejects_background_library_actions() {
    let mut app = app();
    app.search = "original".into();
    let _ = app.update(Message::Add);
    let _ = app.update(Message::SearchChanged("hidden".into()));
    let _ = app.update(Message::Select("other".into()));
    let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
    let _ = app.update(Message::Play);
    assert_eq!(app.search, "original");
    assert!(app.selected.is_none());
    assert!(app.settings.is_none());
    assert!(app.operation.is_none());
    assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
}

#[test]
fn flow_result_selects_new_card_even_when_search_hides_it() {
    let mut app = app();
    app.search = "other".into();
    let _ = app.flow_returned(
        flow::Action::Library {
            name: Some("new".into()),
            launch: false,
        },
        Ok(vec![card("new", State::Inactive, 0)]),
    );
    assert!(app.search.is_empty());
    assert_eq!(app.selected.as_deref(), Some("new"));
    assert_eq!(app.target.as_deref(), Some("game"));
    assert!(app.panel_open);
    assert!(app.operation.is_none());
}

#[test]
fn closing_ignores_pending_launch_and_stale_flow_replies() {
    let mut app = app();
    app.flow = Some(flow::Page::choose(app.paths.clone()));
    app.flow_generation = 5;
    let _ = app.flow_update(4, flow::Event::New);
    assert!(matches!(app.flow, Some(flow::Page::Choose(_))));
    let _ = app.update(Message::FlowReturned(
        4,
        flow::Action::Settings {
            name: "old".into(),
            launch: true,
        },
        Ok(vec![card("old", State::Inactive, 0)]),
    ));
    assert!(app.selected.is_none());
    assert!(app.settings.is_none());
    app.closing = true;
    let _ = app.flow_returned(
        flow::Action::Library {
            name: Some("new".into()),
            launch: true,
        },
        Ok(vec![card("new", State::Inactive, 0)]),
    );
    assert!(app.operation.is_none());
    assert!(app.selected.is_none());
}
#[test]
fn search_is_case_insensitive() {
    let mut app = app();
    let mut named = card("dusk_id", State::Inactive, 0);
    named.display_title = "Dusklight".into();
    app.cards = vec![named, card("Other", State::Inactive, 0)];
    app.search = "LIGHT".into();
    assert_eq!(app.filtered().len(), 1);
    assert_eq!(app.filtered()[0].display_title, "Dusklight");
    assert_eq!(app.filtered()[0].name, "dusk_id");
    app.search = "DUSK_ID".into();
    assert_eq!(app.filtered().len(), 1);
    assert_eq!(app.filtered()[0].name, "dusk_id");
}
#[test]
fn card_state_and_playtime() {
    let palette = Palette::DARK;
    let running = card("game", State::Running, 11520);
    assert_eq!(running.state_word(), "running");
    assert_eq!(running.dot(palette), palette.success);
    assert_eq!(running.playtime(), "3 h 12 m");
    let idle = card("game", State::Inactive, 0);
    assert_eq!(idle.dot(palette), palette.muted);
    assert_eq!(idle.playtime(), "never played");
    let failed = card("game", State::Failed, 0);
    assert_eq!(failed.dot(palette), palette.warning);
    let mut busy = card("game", State::Running, 0);
    busy.busy = true;
    assert_eq!(busy.state_word(), "busy");
    assert_eq!(busy.dot(palette), palette.warning);
    busy.error = Some("broken config".into());
    assert_eq!(busy.state_word(), "error");
    assert_eq!(busy.dot(palette), palette.warning);
}
#[test]
fn refresh_preserves_search_and_selection_until_removed() {
    let mut app = app();
    app.search = "ga".into();
    app.selected = Some("game".into());
    app.scroll_offset = 140.0;
    let _ = app.update(Message::Loaded(Ok(vec![card("game", State::Running, 60)])));
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert_eq!(app.search, "ga");
    assert_eq!(app.scroll_offset, 140.0);
    let _ = app.update(Message::Loaded(Ok(vec![card("other", State::Inactive, 0)])));
    assert_eq!(app.selected, None);
}

#[test]
fn changed_art_identity_reloads_and_evicts_old_handle() {
    let mut app = app();
    let old = art_file("/tmp/grid.png", 10);
    let new = art_file("/tmp/grid.png", 20);
    let old_key = (old.clone(), artwork::Kind::Grid);
    let new_key = (new.clone(), artwork::Kind::Grid);
    app.merge(vec![card_with_art(old)]);
    let _ = app.load_visible_art();
    assert!(app.loading.contains(&(old_key.clone(), false)));
    let handle = image::Handle::from_rgba(1, 1, vec![1, 2, 3, 255]);
    let _ = app.update(Message::ArtLoaded(old_key.clone(), false, Ok(handle)));
    assert!(app.images.contains(&old_key, false));
    app.merge(vec![card_with_art(new)]);
    assert!(!app.images.contains(&old_key, false));
    let _ = app.load_visible_art();
    assert!(app.loading.contains(&(new_key.clone(), false)));
    assert!(!app.loading.contains(&(old_key.clone(), false)));
}

#[test]
fn deep_scrolls_load_art_for_every_visible_row_of_mixed_heights() {
    let mut app = app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    let columns = grid_motion::columns_for_viewport(app.viewport_width, false);
    app.cards = (0..columns * 120)
        .map(|index| {
            let mut card = card(&format!("game-{index}"), State::Inactive, 0);
            // Every third row has a titled card, which adds the
            // identifier line and makes that row taller.
            let row = index / columns;
            if row.is_multiple_of(3) && index.is_multiple_of(columns) {
                card.display_title = format!("Game {index}");
            }
            // Wrapped state and playtime labels make rows taller too.
            if row % 5 == 1 && index % columns == 1 {
                card.state = State::SteamRunning;
            }
            if row % 7 == 2 && index % columns == 2 {
                card.playtime = "1234 hours 56 minutes across many sessions".to_owned();
            }
            card.art = Some(art_file(&format!("/tmp/art-{index}.png"), index as u64));
            card
        })
        .collect();
    let row_heights: Vec<f32> = app
        .cards
        .chunks(columns)
        .map(|row| row.iter().map(grid::card_height).fold(0.0, f32::max))
        .collect();
    assert_ne!(row_heights[0], row_heights[1]);
    let row_top = |row: usize| 16.0 + row_heights[..row].iter().map(|h| h + GAP).sum::<f32>();
    app.scroll_offset = row_top(60);
    let _ = app.load_visible_art();
    let bottom = app.scroll_offset + app.viewport_height;
    let visible: Vec<usize> = (0..row_heights.len())
        .filter(|row| {
            row_top(*row) <= bottom && row_top(*row) + row_heights[*row] >= app.scroll_offset
        })
        .collect();
    assert_eq!(visible.first(), Some(&60));
    for row in visible {
        for card in &app.cards[row * columns..(row + 1) * columns] {
            let key = (card.art.clone().unwrap(), artwork::Kind::Grid);
            assert!(
                app.loading.contains(&(key.clone(), false)),
                "row {row} wasn't loaded"
            );
        }
    }
    // The range stays near the viewport instead of loading everything.
    assert!(app.loading.len() < 20 * columns);
}

#[test]
fn steam_install_changes_swap_the_card_between_color_and_grayscale() {
    let mut app = app();
    let art = art_file("/tmp/steam-grid.png", 10);
    let key = (art.clone(), artwork::Kind::Grid);
    let mut card = card_with_art(art);
    card.kind = Some(crate::library::CardKind::Proxied);
    card.steam = Some(crate::library::SteamDetails {
        install: wineo_core::steam::AppState::Installed.into(),
        proton: None,
        size: None,
    });
    app.merge(vec![card.clone()]);
    let color = image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    let gray = image::Handle::from_rgba(1, 1, vec![70, 70, 70, 255]);
    let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(color.clone())));
    assert!(!app.images.contains(&key, true));

    card.steam.as_mut().unwrap().install = wineo_core::steam::AppState::NotInstalled.into();
    app.merge(vec![card.clone()]);
    // The color copy stands in while the grayscale one decodes.
    assert_eq!(app.images.shown(&key, true), Some(&color));
    let _ = app.load_visible_art();
    assert!(app.loading.contains(&(key.clone(), true)));
    let _ = app.update(Message::ArtLoaded(key.clone(), true, Ok(gray.clone())));
    assert_eq!(app.images.shown(&key, true), Some(&gray));
    assert!(!app.images.contains(&key, false));

    card.steam.as_mut().unwrap().install =
        wineo_core::steam::AppState::Unknown("Missing Steam".to_owned()).into();
    app.merge(vec![card]);
    assert_eq!(app.images.shown(&key, false), Some(&gray));
    let _ = app.load_visible_art();
    assert!(app.loading.contains(&(key.clone(), false)));
    let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(color.clone())));
    assert_eq!(app.images.shown(&key, false), Some(&color));
    assert!(!app.images.contains(&key, true));
    app.merge(Vec::new());
    assert_eq!(app.images.len(), 0);
}

#[test]
fn a_variant_decoded_after_its_card_switched_back_is_dropped() {
    let mut app = app();
    let art = art_file("/tmp/steam-grid.png", 10);
    let key = (art.clone(), artwork::Kind::Grid);
    let mut card = steam_card("game", wineo_core::steam::AppState::Installed);
    card.art = Some(art);
    app.merge(vec![card.clone()]);
    let color = image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(color.clone())));
    card.steam.as_mut().unwrap().install = wineo_core::steam::AppState::NotInstalled.into();
    app.merge(vec![card.clone()]);
    let _ = app.load_visible_art();
    card.steam.as_mut().unwrap().install = wineo_core::steam::AppState::Installed.into();
    app.merge(vec![card]);
    let _ = app.update(Message::ArtLoaded(
        key.clone(),
        true,
        Ok(image::Handle::from_rgba(1, 1, vec![70, 70, 70, 255])),
    ));
    assert_eq!(app.images.shown(&key, false), Some(&color));
    assert!(!app.images.contains(&key, true));
}

#[test]
fn dimmed_art_decodes_to_grayscale_off_the_ui_thread() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("grid.png");
    ::image::RgbaImage::from_pixel(2, 3, ::image::Rgba([10, 80, 240, 255]))
        .save(&path)
        .unwrap();
    let key = (art_file(path.to_str().unwrap(), 1), artwork::Kind::Grid);
    let pixels = |handle: image::Handle| match handle {
        image::Handle::Rgba { pixels, .. } => pixels[..4].to_vec(),
        _ => panic!("expected decoded art"),
    };
    let cache = root.path().join("cache");
    assert_eq!(
        pixels(decode_art(&key, false, &cache).unwrap()),
        [10, 80, 240, 255]
    );
    let gray = pixels(decode_art(&key, true, &cache).unwrap());
    assert!(gray[0] == gray[1] && gray[1] == gray[2] && gray[3] == 255);
}

#[test]
fn art_scrolled_out_of_range_is_evicted_and_reloads_when_it_returns() {
    let mut app = app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    let columns = grid_motion::columns_for_viewport(app.viewport_width, false);
    app.merge(
        (0..columns * 60)
            .map(|index| {
                let mut card = card(&format!("game-{index}"), State::Inactive, 0);
                card.art = Some(art_file(&format!("/tmp/art-{index}.png"), index as u64));
                card
            })
            .collect(),
    );
    let poster = 600 * 1024;
    let load = |app: &mut App| {
        let _ = app.load_visible_art();
        let keys: Vec<_> = app.loading.drain().collect();
        for (key, dimmed) in &keys {
            let handle = image::Handle::from_rgba(1, poster / 4, vec![0; poster as usize]);
            let _ = app.update(Message::ArtLoaded(key.clone(), *dimmed, Ok(handle)));
        }
        keys
    };
    let middle_offset = 30.0 * 320.0;
    let top = load(&mut app);
    app.scroll_offset = middle_offset;
    let middle = load(&mut app);
    assert!(!top.is_empty() && middle.iter().all(|key| !top.contains(key)));
    // Room for either range around the viewport, but not both.
    let budget = top.len().max(middle.len()) * poster as usize;
    app.images = art_cache::ArtCache::with_budget(budget);
    app.scroll_offset = 0.0;
    load(&mut app);
    app.scroll_offset = middle_offset;
    load(&mut app);
    for (key, dimmed) in &middle {
        assert!(app.images.contains(key, *dimmed));
    }
    let evicted = top
        .iter()
        .filter(|(key, dimmed)| !app.images.contains(key, *dimmed))
        .count();
    assert!(evicted > 0);
    assert!(app.images.bytes() <= budget);

    app.scroll_offset = 0.0;
    let _ = app.load_visible_art();
    for key in &top {
        assert!(app.images.contains(&key.0, key.1) || app.loading.contains(key));
    }
    assert_eq!(app.loading.len(), evicted);
}

fn steam_card(name: &str, install: wineo_core::steam::AppState) -> Card {
    let mut card = card(name, State::Inactive, 0);
    card.kind = Some(crate::library::CardKind::Proxied);
    card.tags = vec!["game".into()];
    card.targets = vec!["steam".into()];
    card.default_target = Some("steam".into());
    card.steam = Some(crate::library::SteamDetails {
        install: install.into(),
        proton: None,
        size: None,
    });
    card
}

#[test]
fn the_steam_key_link_opens_global_settings_at_the_key_row() {
    let mut app = app();
    let _ = app.update(Message::Add);
    let generation = app.flow_generation;
    let _ = app.flow_update(generation, flow::Event::Steam);
    assert!(app.flow.is_some());
    app.flow = None;
    let _ = app.open_steam_key_settings();
    let page = app.settings.as_ref().unwrap();
    assert!(page.is_global());
    assert!(page.reveal_steam_key);
}

#[test]
fn install_replaces_play_and_asks_steam_once() {
    use wineo_core::steam::AppState;
    let mut app = app();
    app.merge(vec![
        steam_card("owned", AppState::NotInstalled),
        steam_card("queued", AppState::InstallQueued),
        steam_card("installed", AppState::Installed),
    ]);
    for (name, installable) in [("owned", true), ("queued", true), ("installed", false)] {
        let card = app.cards.iter().find(|card| card.name == name).unwrap();
        assert_eq!(card.installable(), installable, "{name}");
        assert_eq!(panel::install_enabled(card, false), installable, "{name}");
        assert!(!panel::install_enabled(card, true), "{name}");
    }

    let _ = app.update(Message::Select("installed".into()));
    assert_eq!(app.update(Message::Install).units(), 0);
    assert!(app.steam_request.is_none());

    let _ = app.update(Message::Select("owned".into()));
    app.panel_note = Some("old".into());
    assert!(app.update(Message::Install).units() > 0);
    assert_eq!(app.steam_request.as_deref(), Some("owned"));
    assert!(app.panel_note.is_none());
    assert!(app.spinning());
    // A second press waits for Steam's answer.
    assert_eq!(app.update(Message::Install).units(), 0);

    let _ = app.update(Message::SteamRequested(
        "owned".into(),
        Ok("Asked Steam to install owned.".into()),
    ));
    assert!(app.steam_request.is_none());
    assert_eq!(
        app.panel_note.as_deref(),
        Some("Asked Steam to install owned.")
    );
    assert!(!app.spinning());
}

#[test]
fn start_stays_disabled_until_steam_has_the_game() {
    use wineo_core::steam::AppState;
    for (install, playable) in [
        (AppState::NotInstalled, false),
        (AppState::InstallQueued, false),
        (AppState::Installing(Some(40)), false),
        (AppState::Installing(None), false),
        (AppState::Uninstalling, false),
        (AppState::Updating, true),
        (AppState::Installed, true),
    ] {
        let label = format!("{install:?}");
        let mut app = app();
        app.merge(vec![steam_card("game", install)]);
        let card = &app.cards[0];
        assert_eq!(
            panel::play_enabled(card, false, Some("steam")),
            playable,
            "{label}"
        );
        let _ = app.update(Message::Select("game".into()));
        let _ = app.update(Message::Play);
        assert_eq!(app.operation.is_some(), playable, "{label}");
    }
}

#[test]
fn steam_request_refusals_show_in_the_panel() {
    let mut app = app();
    app.merge(vec![steam_card(
        "game",
        wineo_core::steam::AppState::Installed,
    )]);
    let _ = app.update(Message::Select("game".into()));
    app.steam_request = Some("game".into());
    let _ = app.update(Message::SteamRequested(
        "game".into(),
        Err(PanelError::from_error(&anyhow::anyhow!(
            "Stop game before uninstalling it."
        ))),
    ));
    assert!(app.steam_request.is_none());
    assert_eq!(
        app.panel_error.as_ref().unwrap().text,
        "Stop game before uninstalling it."
    );
}

#[test]
fn uninstall_is_offered_only_for_installed_steam_games() {
    use wineo_core::steam::AppState;
    let mut app = app();
    let mut wine = card("wine", State::Inactive, 0);
    wine.kind = Some(crate::library::CardKind::Wine);
    wine.prefix = Some(PathBuf::from("/tmp/wine-prefix"));
    let mut native = card("native", State::Inactive, 0);
    native.kind = Some(crate::library::CardKind::Native);
    let mut broken = steam_card("broken", AppState::Installed);
    broken.error = Some("broken config".into());
    app.merge(vec![
        steam_card("installed", AppState::Installed),
        steam_card("updating", AppState::Updating),
        steam_card("owned", AppState::NotInstalled),
        steam_card("queued", AppState::InstallQueued),
        steam_card("installing", AppState::Installing(Some(5))),
        steam_card("uninstalling", AppState::Uninstalling),
        steam_card("missing", AppState::Unknown("Steam isn't found".into())),
        broken,
        wine,
        native,
    ]);
    for (name, offered) in [
        ("installed", true),
        ("updating", true),
        ("owned", false),
        ("queued", false),
        ("installing", false),
        ("uninstalling", false),
        ("missing", false),
        ("broken", false),
        ("wine", false),
        ("native", false),
    ] {
        assert_eq!(app.uninstall_action(name), offered, "{name}");
    }

    assert_eq!(app.update(Message::Uninstall("owned".into())).units(), 0);
    assert!(app.steam_request.is_none());

    // The menu's Uninstall selects the game, so Steam's answer has a place.
    app.action_menu = Some("installed".into());
    assert!(app.update(Message::Uninstall("installed".into())).units() > 0);
    assert!(app.action_menu.is_none());
    assert_eq!(app.selected.as_deref(), Some("installed"));
    assert!(app.panel_open);
    assert_eq!(app.steam_request.as_deref(), Some("installed"));
    assert_eq!(app.update(Message::Uninstall("updating".into())).units(), 0);
}

#[test]
fn steam_requests_use_the_install_path_and_its_notes() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let steamapps = root.path().join(".local/share/Steam/steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::create_dir_all(paths.environments_dir()).unwrap();
    std::fs::write(
        paths.environment_config("native"),
        "kind = 'native'
",
    )
    .unwrap();
    std::fs::write(
        paths.environment_config("game"),
        "kind = 'proxied'\nlauncher = 'steam'\napp_id = '123'\n",
    )
    .unwrap();
    let request = |name, action| {
        worker::steam_request_now(&paths, name, action).map_err(|error| error.to_string())
    };
    use wineo_core::steam_install::Action;
    assert_eq!(
        request("native", Action::Install).unwrap_err(),
        "Only Steam games can be installed."
    );
    assert_eq!(
        request("game", Action::Uninstall).unwrap(),
        "Not installed."
    );
    std::fs::write(
        steamapps.join("appmanifest_123.acf"),
        r#""AppState" { "appid" "123" "name" "Game" "StateFlags" "4" }"#,
    )
    .unwrap();
    assert_eq!(
        request("game", Action::Install).unwrap(),
        "Already installed."
    );
    std::fs::write(
        steamapps.join("appmanifest_123.acf"),
        r#""AppState" { "appid" "123" "name" "Game" "StateFlags" "1026" }"#,
    )
    .unwrap();
    assert_eq!(
        request("game", Action::Install).unwrap(),
        "Steam is already installing game."
    );
}

#[test]
fn opening_the_panel_keeps_art_for_cards_sweeping_through_the_viewport() {
    let mut app = app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    app.merge(
        (0..300)
            .map(|index| {
                let mut card = card(&format!("game-{index}"), State::Inactive, 0);
                card.art = Some(art_file(&format!("/tmp/art-{index}.png"), index as u64));
                card
            })
            .collect(),
    );
    app.scroll_offset = 9600.0;
    let heights: Vec<f32> = app.cards.iter().map(grid::card_height).collect();
    let margin = 2.0 * (heights[0] + GAP);
    let range = |open| {
        grid_motion::cards_between(
            &heights,
            grid_motion::columns_for_viewport(app.viewport_width, open),
            app.scroll_offset - margin,
            app.scroll_offset + app.viewport_height + margin,
        )
    };
    let (closed, open) = (range(false), range(true));
    assert!(open.end < closed.start);
    // Above the viewport when closed and below it when open.
    let crossing = (open.end + closed.start) / 2;
    let key = (
        app.cards[crossing].art.clone().unwrap(),
        artwork::Kind::Grid,
    );
    let _ = app.load_visible_art();
    assert!(!app.visible_art.contains(&key));

    let _ = app.update(Message::Select(app.cards[closed.start].name.clone()));
    assert!(app.grid_motion.active());
    assert!(app.visible_art.contains(&key));
    assert!(app.loading.contains(&(key.clone(), false)));

    let generation = app.grid_motion.generation();
    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    assert!(!app.grid_motion.active());
    assert!(!app.visible_art.contains(&key));
}

#[test]
fn closing_the_panel_near_the_bottom_keeps_art_for_cards_rising_into_view() {
    let mut app = app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    app.merge(
        (0..300)
            .map(|index| {
                let mut card = card(&format!("game-{index}"), State::Inactive, 0);
                card.art = Some(art_file(&format!("/tmp/art-{index}.png"), index as u64));
                card
            })
            .collect(),
    );
    let _ = app.update(Message::Select("game-250".into()));
    let generation = app.grid_motion.generation();
    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    assert!(app.panel_open && !app.grid_motion.active());
    app.scroll_offset = 25000.0;
    let heights: Vec<f32> = app.cards.iter().map(grid::card_height).collect();
    let margin = 2.0 * (heights[0] + GAP);
    let range = |open| {
        grid_motion::cards_between(
            &heights,
            grid_motion::columns_for_viewport(app.viewport_width, open),
            app.scroll_offset - margin,
            app.scroll_offset + app.viewport_height + margin,
        )
    };
    // The closed layout ends above the viewport.
    assert!(range(false).is_empty());
    let open = range(true);
    assert!(!open.is_empty() && open.end < 270);
    let key = (app.cards[270].art.clone().unwrap(), artwork::Kind::Grid);
    app.images = art_cache::ArtCache::with_budget(0);

    let _ = app.update(Message::ClosePanel);
    assert!(app.grid_motion.active());
    assert!(app.visible_art.contains(&key));
    assert!(app.loading.contains(&(key.clone(), false)));
    let pixel = image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(pixel)));
    assert!(app.images.contains(&key, false));

    let generation = app.grid_motion.generation();
    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    assert!(!app.visible_art.contains(&key));
    assert!(!app.images.contains(&key, false));
}

#[test]
fn a_closing_panel_keeps_drawing_its_picked_target_until_the_exit_ends() {
    let mut app = app();
    let art = |target: &str| crate::library::ArtPaths {
        hero: Some(art_file(&format!("/tmp/{target}-hero.png"), 1)),
        logo: Some(art_file(&format!("/tmp/{target}-logo.png"), 2)),
    };
    let mut game = card("game", State::Inactive, 0);
    game.targets = vec!["a".into(), "b".into()];
    game.default_target = Some("a".into());
    game.panel_art.insert("a".into(), art("a"));
    game.panel_art.insert("b".into(), art("b"));
    let other = card("other", State::Inactive, 0);
    app.merge(vec![game, other]);
    let keys = |target: &str| {
        let paths = art(target);
        [
            (paths.hero.unwrap(), artwork::Kind::Hero),
            (paths.logo.unwrap(), artwork::Kind::Logo),
        ]
    };
    let pixel = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    // Nothing fits, so only protected art stays.
    app.images = art_cache::ArtCache::with_budget(0);
    let _ = app.update(Message::Select("game".into()));
    // Settled open, so closing starts a real exit.
    let generation = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    for key in keys("a") {
        let _ = app.update(Message::ArtLoaded(key, false, Ok(pixel())));
    }
    let _ = app.update(Message::TargetPicked("b".into()));
    for key in keys("b") {
        let _ = app.update(Message::ArtLoaded(key, false, Ok(pixel())));
    }
    for key in keys("a") {
        assert!(!app.images.contains(&key, false));
    }

    let _ = app.update(Message::ClosePanel);
    assert!(app.target.is_none() && app.panel_motion.active());
    assert_eq!(app.drawn_target(&app.cards[0]), Some("b"));
    let drawn: Vec<_> = app
        .panel_art_keys()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(drawn, keys("b"));
    app.evict_art();
    let paths = art("b");
    let (hero, logo) = app.panel_images(&app.cards[0], paths.hero.as_ref(), paths.logo.as_ref());
    assert!(hero.is_some() && logo.is_some());
    // A selection moved by keys mid-exit draws that card, as before.
    app.selected = Some("other".into());
    assert!(app.panel_art_keys().is_empty());
    app.selected = Some("game".into());

    // Reopening mid-exit starts from the default target, as before.
    let _ = app.update(Message::Select("game".into()));
    assert_eq!(app.drawn_target(&app.cards[0]), Some("a"));
    let _ = app.update(Message::ClosePanel);
    let generation = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    assert!(!app.panel_motion.reserved());
    assert!(app.panel_motion.exit_target().is_none());
    assert!(app.panel_art_keys().is_empty());
}

#[test]
fn an_exit_that_ends_without_a_tick_forgets_its_target() {
    let mut app = app();
    app.viewport_width = 1000.0;
    app.viewport_height = 800.0;
    let mut game = card("game", State::Inactive, 0);
    game.targets = vec!["a".into(), "b".into()];
    game.default_target = Some("a".into());
    app.merge(vec![game]);
    let close_on_b = |app: &mut App| {
        let _ = app.update(Message::Select("game".into()));
        let generation = app.panel_motion.generation();
        let _ = app.update(Message::PanelTick(
            generation,
            Instant::now() + Duration::from_secs(1),
        ));
        let _ = app.update(Message::TargetPicked("b".into()));
        let _ = app.update(Message::ClosePanel);
        assert!(app.panel_motion.active());
        assert_eq!(app.drawn_target(&app.cards[0]), Some("b"));
    };

    close_on_b(&mut app);
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    assert!(app.panel_motion.exit_target().is_none());
    let _ = app.update(Message::Section(sidebar::Section::Library));

    close_on_b(&mut app);
    app.start_collapse(Instant::now());
    assert!(!app.collapse.idle());
    assert!(app.panel_motion.exit_target().is_none());
    assert_eq!(app.drawn_target(&app.cards[0]), Some("a"));
}

#[test]
fn a_closing_panel_keeps_its_art_until_the_exit_ends() {
    use wineo_core::steam::AppState;
    let mut app = app();
    let hero = art_file("/tmp/steam-hero.png", 10);
    let logo = art_file("/tmp/steam-logo.png", 11);
    let other = art_file("/tmp/other-grid.png", 12);
    let mut card = steam_card("game", AppState::Installed);
    card.panel_art.insert(
        "steam".into(),
        crate::library::ArtPaths {
            hero: Some(hero.clone()),
            logo: Some(logo.clone()),
        },
    );
    let mut offscreen = card_with_art(other.clone());
    offscreen.name = "other".into();
    app.merge(vec![card, offscreen]);
    // Nothing fits, so only protected art stays.
    app.images = art_cache::ArtCache::with_budget(0);
    let _ = app.update(Message::Select("game".into()));
    // Settled open, so closing starts a real exit.
    let generation = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        generation,
        Instant::now() + Duration::from_secs(1),
    ));
    let pixel = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    let keys = [
        (hero.clone(), artwork::Kind::Hero),
        (logo.clone(), artwork::Kind::Logo),
    ];
    for key in &keys {
        let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(pixel())));
        assert!(app.images.contains(key, false));
    }

    let _ = app.update(Message::ClosePanel);
    assert!(!app.panel_open && app.panel_motion.active());
    let other_key = (other, artwork::Kind::Grid);
    let _ = app.update(Message::ArtLoaded(other_key.clone(), false, Ok(pixel())));
    for key in &keys {
        assert!(app.images.contains(key, false));
    }

    app.panel_motion.snap(false);
    app.evict_art();
    for key in &keys {
        assert!(!app.images.contains(key, false));
    }
}

#[test]
fn panel_art_is_dimmed_for_games_steam_doesnt_have() {
    use wineo_core::steam::AppState;
    let mut app = app();
    let hero = art_file("/tmp/steam-hero.png", 10);
    let logo = art_file("/tmp/steam-logo.png", 11);
    let mut card = steam_card("game", AppState::NotInstalled);
    card.panel_art.insert(
        "steam".into(),
        crate::library::ArtPaths {
            hero: Some(hero.clone()),
            logo: Some(logo.clone()),
        },
    );
    app.merge(vec![card.clone()]);
    app.selected = Some("game".into());
    app.panel_open = true;
    let keys = [
        (hero.clone(), artwork::Kind::Hero),
        (logo.clone(), artwork::Kind::Logo),
    ];
    let _ = app.load_panel_art();
    let blue = || image::Handle::from_rgba(1, 1, vec![10, 80, 240, 255]);
    let gray = || image::Handle::from_rgba(1, 1, vec![70, 70, 70, 255]);
    for key in &keys {
        assert!(app.loading.contains(&(key.clone(), true)));
        let _ = app.update(Message::ArtLoaded(key.clone(), true, Ok(gray())));
        assert!(!app.images.contains(key, false));
    }
    let is_gray = |handle: Option<&image::Handle>| match handle {
        Some(image::Handle::Rgba { pixels, .. }) => {
            pixels[0] == pixels[1] && pixels[1] == pixels[2]
        }
        _ => panic!("expected decoded art"),
    };
    let (shown_hero, shown_logo) = app.panel_images(&app.cards[0], Some(&hero), Some(&logo));
    assert!(is_gray(shown_hero));
    assert!(is_gray(shown_logo));

    card.steam.as_mut().unwrap().install = AppState::Installed.into();
    app.merge(vec![card]);
    let _ = app.load_panel_art();
    for key in &keys {
        assert!(app.loading.contains(&(key.clone(), false)));
        let _ = app.update(Message::ArtLoaded(key.clone(), false, Ok(blue())));
        assert!(!app.images.contains(key, true));
    }
    let (shown_hero, shown_logo) = app.panel_images(&app.cards[0], Some(&hero), Some(&logo));
    assert!(!is_gray(shown_hero));
    assert!(!is_gray(shown_logo));
}

#[test]
fn failed_art_retries_only_after_identity_changes() {
    let mut app = app();
    let old = art_file("/tmp/broken.png", 10);
    let new = art_file("/tmp/broken.png", 20);
    let old_key = (old.clone(), artwork::Kind::Grid);
    let new_key = (new.clone(), artwork::Kind::Grid);
    app.merge(vec![card_with_art(old)]);
    let _ = app.load_visible_art();
    let _ = app.update(Message::ArtLoaded(
        old_key.clone(),
        false,
        Err("invalid".into()),
    ));
    let _ = app.load_visible_art();
    assert!(app.failed_images.contains(&old_key));
    assert!(app.loading.is_empty());
    app.merge(vec![card_with_art(new)]);
    assert!(!app.failed_images.contains(&old_key));
    let _ = app.load_visible_art();
    assert!(app.loading.contains(&(new_key.clone(), false)));
}

#[test]
fn library_keeps_native_and_broken_configs() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::from_lookup(|key| match key {
        "HOME" | "XDG_CONFIG_HOME" | "XDG_DATA_HOME" | "XDG_STATE_HOME" => {
            Some(root.path().as_os_str().to_owned())
        }
        _ => None,
    })
    .unwrap();
    std::fs::create_dir_all(paths.environments_dir()).unwrap();
    std::fs::write(
        paths.environment_config("native"),
        "schema_version = 1\nkind = \"native\"\n[targets.game]\nexe = \"missing-game\"\ndefault = true\n",
    )
    .unwrap();
    std::fs::write(paths.environment_config("broken"), "this is [").unwrap();
    let cards = load_library(&paths).unwrap();
    assert_eq!(cards.len(), 2);
    assert!(
        cards
            .iter()
            .any(|card| card.name == "native" && card.error.is_none())
    );
    assert!(
        cards
            .iter()
            .any(|card| card.name == "broken" && card.error.is_some())
    );
}

#[test]
fn play_target_uses_default_single_target_or_picker_choice() {
    let mut app = app();
    let mut one = card("one", State::Inactive, 0);
    one.targets = vec!["only".into()];
    one.default_target = Some("only".into());
    assert_eq!(app.chosen_target(&one), Some("only"));
    let mut several = card("many", State::Inactive, 0);
    several.targets = vec!["a".into(), "b".into()];
    several.default_target = Some("b".into());
    assert_eq!(app.chosen_target(&several), Some("b"));
    app.target = Some("a".into());
    assert_eq!(app.chosen_target(&several), Some("a"));
}

#[test]
fn double_click_opens_panel_and_requests_start() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    assert!(app.panel_open);
    let _ = app.update(Message::Select("game".into()));
    assert!(!app.panel_open);
    let _ = app.update(Message::DoubleClick("game".into()));
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open);
    assert_eq!(app.target.as_deref(), Some("game"));
    assert!(app.operation.as_ref().is_some_and(|operation| {
        operation.name == "game" && operation.kind == OperationKind::Start
    }));
}

fn mouse_button(button: iced::mouse::Button) -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(button))
}

fn back_key(status: iced::event::Status) -> Message {
    event_message(mouse_button(iced::mouse::Button::Back), status)
        .expect("Back should map to a key message")
}

/// Delivers a mouse Back press to a page that handles keys directly.
pub(crate) fn press_back(app: &mut App, status: iced::event::Status) {
    let _ = app.update(back_key(status));
}

/// Delivers a mouse Back press on the library, where the Key message
/// continues as KeyWithFocus once the search focus is known.
fn press_back_in_library(app: &mut App, status: iced::event::Status) {
    let Message::Key(key, modifiers, status) = back_key(status) else {
        panic!("Back should map to a key message");
    };
    let _ = app.update(Message::Key(key.clone(), modifiers, status));
    let _ = app.update(Message::KeyWithFocus(key, modifiers, status, false));
}

#[test]
fn mouse_back_maps_to_uncaptured_escape_and_forward_does_nothing() {
    for status in [iced::event::Status::Ignored, iced::event::Status::Captured] {
        assert!(matches!(
            event_message(mouse_button(iced::mouse::Button::Back), status),
            Some(Message::Key(
                keyboard::Key::Named(keyboard::key::Named::Escape),
                modifiers,
                iced::event::Status::Ignored,
            )) if modifiers.is_empty()
        ));
        for button in [
            iced::mouse::Button::Forward,
            iced::mouse::Button::Left,
            iced::mouse::Button::Right,
        ] {
            assert!(event_message(mouse_button(button), status).is_none());
        }
    }
}

#[test]
fn mouse_back_closes_log_then_panel_then_clears_search() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    app.log_view = true;
    app.search = "ga".into();
    // A captured press must still act, like an uncaptured Escape.
    press_back_in_library(&mut app, iced::event::Status::Captured);
    assert!(!app.log_view);
    assert!(app.panel_open);
    press_back_in_library(&mut app, iced::event::Status::Ignored);
    assert!(!app.panel_open);
    assert_eq!(app.search, "ga");
    press_back_in_library(&mut app, iced::event::Status::Ignored);
    assert!(app.search.is_empty());
}

#[test]
fn closing_panel_cancels_log_before_motion_finishes() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    let opened = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        opened,
        Instant::now() + Duration::from_millis(200),
    ));
    assert!(app.panel_motion.interactive());
    let token = Token::new();
    app.log_token = Some(token.clone());
    app.log_view = true;

    let _ = app.update(Message::ClosePanel);
    assert!(!app.panel_open);
    assert!(!app.log_view);
    assert!(token.is_canceled());
    assert!(app.panel_reserved());
    assert!(!app.panel_motion.interactive());
    let closing = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        closing,
        Instant::now() + Duration::from_millis(200),
    ));
    assert!(!app.panel_reserved());
}

#[test]
fn changing_selection_does_not_replay_panel_entrance() {
    let mut app = app();
    app.cards = vec![
        card("one", State::Inactive, 0),
        card("two", State::Inactive, 0),
    ];
    let _ = app.update(Message::Select("one".into()));
    let opened = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        opened,
        Instant::now() + Duration::from_millis(200),
    ));
    let _ = app.update(Message::Select("two".into()));
    assert_eq!(app.selected.as_deref(), Some("two"));
    assert_eq!(app.panel_motion.generation(), opened);
    assert!(app.panel_motion.interactive());
}

#[test]
fn grid_motion_keeps_scroll_tree_and_ignores_stale_bounds() {
    let mut app = app();
    app.cards = (1..=30)
        .map(|index| card(&format!("game-{index}"), State::Inactive, 0))
        .collect();
    let _ = app.update(Message::Select("game-21".into()));
    assert!(app.grid_motion.active());
    let generation = app.grid_motion.generation();
    let _ = app.scroll_selection();
    assert_eq!(app.grid_scroll_after_settle.as_deref(), Some("game-21"));

    let bounds = grid_motion::CardBounds {
        card: iced::Rectangle::new(
            iced::Point::new(32.0, 1650.0),
            iced::Size::new(160.0, 300.0),
        ),
        viewport: iced::Rectangle::new(iced::Point::new(0.0, 50.0), iced::Size::new(800.0, 600.0)),
        offset: iced::Vector::ZERO,
    };
    let _ = app.update(Message::GridCardLocated(
        "game-21".into(),
        generation,
        bounds,
    ));
    assert_eq!(app.scroll_offset, 1300.0);
    let _ = app.update(Message::GridCardLocated(
        "game-21".into(),
        generation - 1,
        grid_motion::CardBounds {
            card: iced::Rectangle::new(iced::Point::new(0.0, 0.0), iced::Size::new(160.0, 300.0)),
            ..bounds
        },
    ));
    assert_eq!(app.scroll_offset, 1300.0);

    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_millis(310),
    ));
    assert!(!app.grid_motion.active());
    assert!(app.grid_scroll_after_settle.is_none());
}

#[test]
fn unchanged_refresh_does_not_cut_grid_motion_short() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    let generation = app.grid_motion.generation();
    let _ = app.update(Message::GridTick(
        generation,
        Instant::now() + Duration::from_millis(90),
    ));
    let position = app.grid_motion.progress(0);
    app.merge(app.cards.clone());
    assert!(app.grid_motion.active());
    assert_eq!(app.grid_motion.generation(), generation);
    assert_eq!(app.grid_motion.progress(0), position);
}

#[test]
fn leaving_library_finishes_motion_and_ignores_stale_tick() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    let stale = app.panel_motion.generation();
    assert!(app.panel_motion.active());
    let _ = app.update(Message::Section(sidebar::Section::Trash));
    assert!(!app.panel_motion.active());
    let _ = app.update(Message::PanelTick(
        stale,
        Instant::now() + Duration::from_secs(1),
    ));
    assert_eq!(app.panel_motion.progress(), 1.0);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert!(app.panel_motion.interactive());
}

#[test]
fn checkup_from_a_closed_panel_starts_its_presentation() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Checkup(checkup::Event::Open(
        checkup::Scope::Environment("game".into()),
    )));
    assert!(app.panel_open);
    assert!(app.panel_reserved());
    assert!(app.panel_motion.active());
    let opened = app.panel_motion.generation();
    let _ = app.update(Message::PanelTick(
        opened,
        Instant::now() + Duration::from_millis(200),
    ));
    assert!(app.panel_motion.interactive());
}

#[test]
fn opening_a_flow_finishes_hidden_panel_motion() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    let _ = app.update(Message::Select("game".into()));
    assert!(app.panel_motion.active());
    let _ = app.update(Message::Add);
    assert!(app.flow.is_some());
    assert!(app.panel_hidden());
    assert!(!app.panel_motion.active());
    assert_eq!(app.panel_motion.progress(), 1.0);
}

#[test]
fn double_click_keeps_open_panels_picker_choice() {
    let mut app = app();
    let mut game = card("game", State::Inactive, 0);
    game.targets = vec!["game".into(), "alternate".into()];
    game.display_title = "My Game".into();
    app.cards = vec![game];
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::TargetPicked("alternate".into()));
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::DoubleClick("game".into()));
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert_eq!(app.cards[0].name, "game");
    assert_eq!(app.cards[0].display_title, "My Game");
    assert!(app.panel_open);
    assert_eq!(app.target.as_deref(), Some("alternate"));
    assert!(app.operation.as_ref().is_some_and(|operation| {
        operation.name == "game" && operation.kind == OperationKind::Start
    }));
}

#[test]
fn double_click_cannot_start_disabled_card() {
    let mut app = app();
    app.cards = vec![card("game", State::Running, 0)];
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::Select("game".into()));
    let _ = app.update(Message::DoubleClick("game".into()));
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open);
    assert!(app.operation.is_none());
}

fn hook_app(phase: wineo_core::hooks::Phase) -> App {
    let mut app = app();
    let mut game = card("game", State::Inactive, 0);
    game.busy = true;
    game.busy_holder = Some(wineo_core::operation::Operation::Hook(phase));
    app.cards = vec![game];
    let _ = app.update(Message::Select("game".into()));
    app
}

fn record_after_hook_identity(paths: &Paths, name: &str) {
    let running = wineo_core::process::Identity {
        group: 12345,
        boot_id: "test-boot".into(),
        leader_start: 67890,
    };
    std::fs::write(
        paths.locks_dir().join(format!("name-{name}.lock")),
        format!(
            "hook-after {} {} {} {}\n",
            std::process::id(),
            running.group,
            running.boot_id,
            running.leader_start
        ),
    )
    .unwrap();
}

#[test]
fn stop_after_opens_confirmation_and_cancel_keeps_hook_busy() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    let _ = app.update(Message::Stop);
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    assert!(app.operation.is_none());
    let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    let _ = app.update(Message::ClosePanel);
    let _ = app.update(Message::Select("other".into()));
    assert_eq!(app.section, sidebar::Section::Library);
    assert!(app.panel_open);
    assert_eq!(app.selected.as_deref(), Some("game"));
    let _ = app.update(Message::CancelHookStop);
    assert!(app.hook_stop_confirm.is_none());
    assert!(app.cards[0].busy);
    assert!(app.operation.is_none());
    let _ = app.update(Message::Stop);
    let _ = app.update(Message::Key(
        keyboard::Key::Named(keyboard::key::Named::Escape),
        keyboard::Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(app.hook_stop_confirm.is_none());
    assert!(app.panel_open);
}

#[test]
fn stop_while_our_own_stop_waits_on_after_opens_confirmation() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.operation = Some(Operation {
        name: "game".into(),
        kind: OperationKind::Stop,
        token: None,
        cancelled: false,
        status: "Stopping...".into(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
    let _ = app.update(Message::Stop);
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
}

#[test]
fn confirm_after_uses_force_stop_only_while_hook_is_active() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.paths = paths;
    let guard = wineo_core::operation::Locks::new(&app.paths)
        .acquire(
            wineo_core::operation::Operation::Hook(wineo_core::hooks::Phase::After),
            "game",
            &[],
        )
        .unwrap();
    record_after_hook_identity(&app.paths, "game");
    let _ = app.update(Message::Stop);
    let task = app.update(Message::ConfirmHookStop);
    assert!(task.units() > 0);
    assert!(app.hook_stop_confirm.is_none());
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
    drop(guard);
    app.operation = None;
    let _ = app.update(Message::Stop);
    let task = app.update(Message::ConfirmHookStop);
    assert_eq!(task.units(), 0);
    assert!(app.operation.is_none());
}

#[test]
fn confirm_after_refreshes_missing_hook_identity() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.paths = paths.clone();
    let _ = app.update(Message::Stop);
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    assert!(app.hook_stop_identity.is_none());

    let guard = wineo_core::operation::Locks::new(&paths)
        .acquire(
            wineo_core::operation::Operation::Hook(wineo_core::hooks::Phase::After),
            "game",
            &[],
        )
        .unwrap();
    record_after_hook_identity(&paths, "game");
    let task = app.update(Message::ConfirmHookStop);
    assert!(task.units() > 0);
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
    drop(task);
    drop(guard);
}

#[test]
fn confirm_after_without_matching_hook_shows_note() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.paths = paths;
    let _ = app.update(Message::Stop);
    assert!(app.hook_stop_identity.is_none());

    let task = app.update(Message::ConfirmHookStop);
    assert_eq!(task.units(), 0);
    assert_eq!(
        app.panel_note.as_deref(),
        Some(worker::AFTER_HOOK_ENDED_NOTE)
    );
    assert!(app.operation.is_none());
}

#[test]
fn reload_dismisses_confirmation_when_after_is_no_longer_busy() {
    for holder in [
        None,
        Some(wineo_core::operation::Operation::Hook(
            wineo_core::hooks::Phase::Before,
        )),
    ] {
        let mut app = hook_app(wineo_core::hooks::Phase::After);
        let _ = app.update(Message::Stop);
        let mut loaded = app.cards[0].clone();
        loaded.busy_holder = holder;
        loaded.busy = holder.is_some();
        let _ = app.update(Message::LibraryReloaded(
            app.library_generation,
            Ok(vec![loaded]),
        ));
        assert!(app.hook_stop_confirm.is_none());
        assert!(app.hook_stop_identity.is_none());
    }
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    let _ = app.update(Message::Stop);
    let _ = app.update(Message::Loaded(Ok(Vec::new())));
    assert!(app.hook_stop_confirm.is_none());
}

#[test]
fn reload_keeps_confirmation_for_an_active_after_hook() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    let _ = app.update(Message::Stop);
    let _ = app.update(Message::Loaded(Ok(app.cards.clone())));
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
}

#[test]
fn stop_turns_into_force_stop_only_after_the_delay() {
    let mut app = selected_app(State::Running);
    let task = app.update(Message::Stop);
    assert!(task.units() > 0);
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
    let (_, at) = app.force_stop_at.clone().unwrap();
    let card = app.cards[0].clone();
    assert!(!app.force_stop_ready(&card, at - Duration::from_millis(1)));
    assert!(app.force_stop_ready(&card, at));

    // A second click inside the delay, like a double-click, does nothing.
    let task = app.update(Message::Stop);
    assert_eq!(task.units(), 0);
    assert!(!app.operation.as_ref().unwrap().cancelled);

    app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
    let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
    let task = app.update(Message::Stop);
    assert!(task.units() > 0);
    let operation = app.operation.as_ref().unwrap();
    assert!(operation.cancelled);
    assert_eq!(operation.status, "Force stopping...");
    assert!(app.force_stop_at.is_none());
    assert_eq!(app.update(Message::Stop).units(), 0);

    let _ = app.update(Message::StopFinished("game".into(), Ok(Vec::new())));
    assert!(app.operation.is_none());
}

#[test]
fn stop_wakes_the_app_when_force_stop_becomes_available() {
    let mut app = selected_app(State::Running);
    // The stop worker plus a one-shot wakeup at the deadline.
    assert_eq!(app.update(Message::Stop).units(), 2);
    let at = Instant::now() + Duration::from_millis(30);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let message = runtime.block_on(crate::panel::force_stop_due(at));
    assert!(matches!(message, Message::ForceStopDue));
    assert!(Instant::now() >= at);
    let _ = app.update(message);
}

#[test]
fn force_stop_waits_for_the_after_hook_confirmation_instead() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.operation = Some(Operation {
        name: "game".into(),
        kind: OperationKind::Stop,
        token: None,
        cancelled: false,
        status: "Stopping...".into(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
    let _ = app.update(Message::Stop);
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    assert!(!app.operation.as_ref().unwrap().cancelled);
}

#[test]
fn steam_waiting_and_client_runs_use_stop_and_force_stop() {
    for state in [State::Starting, State::SteamRunning] {
        let mut app = selected_app(state);
        let card = &mut app.cards[0];
        card.kind = Some(crate::library::CardKind::Proxied);
        card.steam = Some(crate::library::SteamDetails {
            install: wineo_core::steam::AppState::Installed.into(),
            proton: Some("proton_experimental".to_owned()),
            size: Some(2048),
        });
        card.default_target = Some("steam".into());
        card.targets = vec!["steam".into()];
        app.target = Some("steam".into());
        let _ = iced::advanced::widget::Tree::new(app.view().as_widget());
        assert_eq!(app.update(Message::Play).units(), 0);
        assert_eq!(app.remove_reason("game").as_deref(), Some("Stop it first"));
        assert!(app.update(Message::Stop).units() > 0);
        assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
        app.force_stop_at = Some(("game".into(), Instant::now() - Duration::from_secs(1)));
        assert!(app.update(Message::Stop).units() > 0);
        assert_eq!(app.operation.as_ref().unwrap().status, "Force stopping...");
    }
}

#[test]
fn steam_stop_during_handoff_switches_to_the_stop_worker() {
    let mut app = selected_app(State::Starting);
    app.cards[0].kind = Some(crate::library::CardKind::Proxied);
    app.operation = Some(installing_start("game", false));
    assert!(app.update(Message::Stop).units() > 0);
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
    let _ = app.update(Message::StartFinished(
        "game".into(),
        Ok(wineo_core::launch::StartOutcome::SteamRequested),
    ));
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Stop);
}

#[test]
fn stop_during_hookless_start_does_not_cancel_start_token() {
    let mut app = selected_app(State::Running);
    let token = Token::new();
    app.operation = Some(installing_start("game", false));
    app.operation.as_mut().unwrap().token = Some(token.clone());
    let task = app.update(Message::Stop);
    assert_eq!(task.units(), 0);
    assert!(!token.is_canceled());
    assert!(!app.operation.as_ref().unwrap().cancelled);
}

#[test]
fn stop_before_uses_the_launch_cancel_token() {
    let mut app = hook_app(wineo_core::hooks::Phase::Before);
    let token = Token::new();
    app.operation = Some(installing_start("game", false));
    app.operation.as_mut().unwrap().token = Some(token.clone());
    let _ = app.update(Message::Stop);
    assert!(token.is_canceled());
    assert!(app.hook_stop_confirm.is_none());
    assert_eq!(app.operation.as_ref().unwrap().kind, OperationKind::Start);
}

#[test]
fn gui_cancel_interrupts_a_real_before_hook() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
            .unwrap();
    let config = paths.environment_config("game");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, "schema_version = 1\nkind = 'native'\n[hooks]\nbefore = 'sleep 30'\n[targets.game]\nexe = '/missing'\n").unwrap();
    let mut app = hook_app(wineo_core::hooks::Phase::Before);
    app.paths = paths.clone();
    let token = Token::new();
    app.operation = Some(installing_start("game", false));
    app.operation.as_mut().unwrap().token = Some(token.clone());
    let capture = tempfile::tempfile().unwrap();
    let work = std::thread::spawn(move || {
        wineo_core::launch::prepare(
            &paths,
            "game",
            &wineo_core::launch::Overrides {
                target: Some("game".into()),
                ..Default::default()
            },
            &token,
            wineo_core::hooks::Output::File(capture),
            |_| {},
        )
    });
    let locks = wineo_core::operation::Locks::new(&app.paths);
    let deadline = Instant::now() + Duration::from_secs(5);
    while locks.busy_with("game").unwrap()
        != Some(wineo_core::operation::Operation::Hook(
            wineo_core::hooks::Phase::Before,
        ))
    {
        if work.is_finished() || Instant::now() >= deadline {
            app.operation
                .as_ref()
                .unwrap()
                .token
                .as_ref()
                .unwrap()
                .force();
            panic!("before hook did not acquire its lock");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = app.update(Message::CancelStart);
    let error = work
        .join()
        .unwrap()
        .err()
        .expect("before hook should be canceled");
    assert!(matches!(
        error.downcast_ref::<wineo_core::Error>(),
        Some(wineo_core::Error::Canceled("launch"))
    ));
    assert!(locks.busy_with("game").unwrap().is_none());
}

#[test]
fn launcher_after_cleanup_gets_confirmation_and_its_fresh_token() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.cards[0].busy = false;
    app.cards[0].busy_holder = None;
    app.operation = Some(installing_start("game", false));
    let fresh = Token::new();
    let _ = app.update(Message::AfterHookPhase("game".into(), fresh.clone()));
    assert_eq!(
        app.operation.as_ref().unwrap().status,
        "Running its after hook"
    );
    let _ = app.update(Message::Stop);
    assert_eq!(app.hook_stop_confirm.as_deref(), Some("game"));
    assert!(!fresh.is_canceled());
}

#[test]
fn after_cleanup_preserves_canceled_launch_error_suppression() {
    let mut app = hook_app(wineo_core::hooks::Phase::After);
    app.operation = Some(installing_start("game", false));
    let _ = app.update(Message::CancelStart);
    let _ = app.update(Message::AfterHookPhase("game".into(), Token::new()));
    assert!(app.operation.as_ref().unwrap().cancelled);
    let error = anyhow::Error::new(wineo_core::Error::Canceled("launch"));
    let _ = app.update(Message::StartFinished(
        "game".into(),
        Err(PanelError::from_error(&error)),
    ));
    assert!(app.panel_error.is_none());
}

#[test]
fn before_error_keeps_output_in_panel_and_cancel_hides_it() {
    for cancelled in [false, true] {
        let mut app = hook_app(wineo_core::hooks::Phase::Before);
        app.operation = Some(installing_start("game", false));
        app.operation.as_mut().unwrap().cancelled = cancelled;
        let error = PanelError {
            text: "Launch canceled: the before hook exited with code 4.".into(),
            cleanup: false,
            output: Some("last line".into()),
        };
        let _ = app.update(Message::StartFinished("game".into(), Err(error.clone())));
        assert_eq!(app.panel_error, (!cancelled).then_some(error));
        let _ = iced::advanced::widget::Tree::new(app.panel(&app.cards[0]).as_widget());
    }
}

#[test]
fn cancel_then_force_stops_the_same_token() {
    let mut app = app();
    let token = Token::new();
    app.operation = Some(Operation {
        name: "game".into(),
        kind: OperationKind::Start,
        token: Some(token.clone()),
        cancelled: false,
        status: "Starting...".into(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    let _ = app.update(Message::CancelStart);
    assert!(token.is_canceled());
    assert!(!token.is_forced());
    assert!(app.operation.as_ref().unwrap().cancelled);
    let _ = app.update(Message::CancelStart);
    assert!(token.is_forced());
}

fn installing_start(name: &str, installing: bool) -> Operation {
    Operation {
        name: name.into(),
        kind: OperationKind::Start,
        token: Some(Token::new()),
        cancelled: false,
        status: "Installing...".into(),
        install_done: Some(Token::new()),
        installing,
        after_hook: false,
    }
}

fn selected_app(state: State) -> App {
    let mut app = app();
    app.cards = vec![card("game", state, 0)];
    app.selected = Some("game".into());
    app.panel_open = true;
    app
}

#[test]
fn install_phase_message_follows_the_start_operation() {
    let mut app = selected_app(State::Inactive);
    app.operation = Some(installing_start("game", false));
    let _ = app.update(Message::InstallPhase("other".into(), true));
    assert!(!app.operation.as_ref().unwrap().installing);
    let _ = app.update(Message::InstallPhase("game".into(), true));
    assert!(app.operation.as_ref().unwrap().installing);
    let _ = app.update(Message::InstallPhase("game".into(), false));
    assert!(!app.operation.as_ref().unwrap().installing);
}

#[test]
fn log_is_offered_for_an_inactive_card_only_while_installing() {
    let mut app = selected_app(State::Inactive);
    assert!(app.log_source(&app.cards[0]).is_none());
    app.operation = Some(installing_start("game", false));
    assert!(app.log_source(&app.cards[0]).is_none());
    app.operation = Some(installing_start("game", true));
    assert!(matches!(
        app.log_source(&app.cards[0]),
        Some(worker::LogSource::Install(_))
    ));
    app.operation = Some(installing_start("other", true));
    assert!(app.log_source(&app.cards[0]).is_none());
}

#[test]
fn open_log_picks_the_file_for_the_phase() {
    let mut app = selected_app(State::Inactive);
    let _ = app.open_log();
    assert!(!app.log_view);

    app.operation = Some(installing_start("game", true));
    let _ = app.open_log();
    assert!(app.log_view);
    assert!(app.log_install);

    // Once the game runs, the same button shows its output.
    app.close_log();
    app.operation = None;
    app.cards = vec![card("game", State::Running, 0)];
    let _ = app.open_log();
    assert!(app.log_view);
    assert!(!app.log_install);
}

#[test]
fn switching_logs_drops_chunks_from_the_old_one() {
    let mut app = selected_app(State::Inactive);
    app.operation = Some(installing_start("game", true));
    let _ = app.open_log();
    let install = app.log_generation;
    let _ = app.update(Message::LogChunk(install, "install line\n".into()));

    app.operation = None;
    app.cards = vec![card("game", State::Running, 0)];
    let _ = app.open_log();
    assert_ne!(app.log_generation, install);
    let _ = app.update(Message::LogChunk(install, "late install line\n".into()));
    let _ = app.update(Message::LogChunk(app.log_generation, "game line\n".into()));
    assert_eq!(app.log_buffer.display(), "game line\n");
}

#[test]
fn finishing_the_install_keeps_the_open_install_log() {
    let mut app = selected_app(State::Inactive);
    app.operation = Some(installing_start("game", true));
    let _ = app.open_log();
    let generation = app.log_generation;
    let _ = app.update(Message::InstallPhase("game".into(), false));
    let _ = app.update(Message::LogFinished(generation, Ok(())));
    assert!(app.log_view);
    assert!(app.log_install);
}

#[test]
fn refresh_keeps_open_panel_target_and_log() {
    let mut app = app();
    app.cards = vec![card("game", State::Inactive, 0)];
    app.selected = Some("game".into());
    app.panel_open = true;
    app.target = Some("game".into());
    app.log_view = true;
    let _ = app.update(Message::Loaded(Ok(vec![card("game", State::Running, 60)])));
    assert!(app.panel_open);
    assert!(app.log_view);
    assert_eq!(app.target.as_deref(), Some("game"));
    assert_eq!(app.cards[0].state, State::Running);
}

#[test]
fn focused_widget_keeps_its_arrow_keys() {
    let mut app = app();
    app.cards = vec![card("a", State::Inactive, 0), card("b", State::Inactive, 0)];
    app.selected = Some("a".into());
    let right = keyboard::Key::Named(keyboard::key::Named::ArrowRight);
    let _ = app.handle_key(right.clone(), keyboard::Modifiers::empty(), false, true);
    assert_eq!(app.selected.as_deref(), Some("a"));
    let _ = app.handle_key(right, keyboard::Modifiers::empty(), false, false);
    assert_eq!(app.selected.as_deref(), Some("b"));
}

#[test]
fn down_from_search_selects_first_match_then_grid_keys_move() {
    let mut app = app();
    app.cards = vec![
        card("alpha", State::Inactive, 0),
        card("amber", State::Inactive, 0),
    ];
    app.search = "a".into();
    let down = keyboard::Key::Named(keyboard::key::Named::ArrowDown);
    let _ = app.handle_key(down, keyboard::Modifiers::empty(), true, true);
    assert!(app.search_focused);
    assert_eq!(app.selected.as_deref(), Some("alpha"));
    let _ = app.update(Message::SearchFocusChanged(false));
    assert!(!app.search_focused);
    let right = keyboard::Key::Named(keyboard::key::Named::ArrowRight);
    let _ = app.handle_key(
        right,
        keyboard::Modifiers::empty(),
        app.search_focused,
        false,
    );
    assert_eq!(app.selected.as_deref(), Some("amber"));
}

#[test]
fn scrollbar_fade_wakes_once_at_its_deadline() {
    let mut app = app();
    let until = Instant::now() + Duration::from_millis(850);
    app.scroll_visible_until = Some(until);
    let _ = app.update(Message::Hover(None));
    assert_eq!(app.scroll_wake.due(), Some(until));
    let stale = app.scroll_wake.generation().wrapping_sub(1);

    // A wakeup from before the deadline moved does nothing.
    app.scroll_visible_until = Some(Instant::now() - Duration::from_millis(1));
    let _ = app.update(Message::ScrollIdle(stale));
    assert!(app.scroll_visible_until.is_some());
    assert!(app.scroll_wake.due().is_some());

    let live = app.scroll_wake.generation();
    let _ = app.update(Message::ScrollIdle(live));
    assert!(app.scroll_visible_until.is_none());
    assert!(app.scroll_wake.due().is_none());
}

#[test]
fn scrollbar_fade_rearms_when_woken_before_a_later_deadline() {
    let mut app = app();
    let _ = app.update(Message::Hover(None));
    app.scroll_visible_until = Some(Instant::now() + Duration::from_millis(100));
    let _ = app.update(Message::Hover(None));
    let live = app.scroll_wake.generation();
    // Scrolling again moved the deadline out while the sleeper waited.
    let later = Instant::now() + Duration::from_secs(5);
    app.scroll_visible_until = Some(later);
    let _ = app.update(Message::ScrollIdle(live));
    assert_eq!(app.scroll_visible_until, Some(later));
    assert_eq!(app.scroll_wake.due(), Some(later));
}

#[test]
fn a_touch_or_a_key_release_rechecks_toast_focus_but_a_move_does_not() {
    let finger = iced::touch::Finger(0);
    let at = iced::Point::ORIGIN;
    assert!(matches!(
        focus_input(iced::Event::Touch(iced::touch::Event::FingerPressed {
            id: finger,
            position: at
        })),
        Some(Message::FocusRecheck)
    ));
    assert!(
        focus_input(iced::Event::Touch(iced::touch::Event::FingerMoved {
            id: finger,
            position: at
        }))
        .is_none()
    );
    assert!(
        focus_input(iced::Event::Keyboard(keyboard::Event::KeyReleased {
            key: keyboard::Key::Named(keyboard::key::Named::Tab),
            modified_key: keyboard::Key::Named(keyboard::key::Named::Tab),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Tab),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
        }))
        .is_some()
    );
}
