use super::*;
use wineo_core::environment::Environment;

/// The editor that Heroic details and Import prefix embed, at the content
/// width of the narrowest and widest import dialogs.
#[test]
fn the_editor_fits_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let draft = Draft::new();
    // Page padding and the scrollbar gutter come off the dialog width.
    let gutter = crate::flow::heroic::SCROLL_GUTTER;
    for width in [280.0_f32 - 48.0 - gutter, 780.0 - gutter] {
        let mut view: Element<'_, Event> =
            iced::widget::scrollable(draft.view(Palette::DARK, true))
                .height(400)
                .into();
        let probe = probe(&mut view, iced::Size::new(width, 400.0));
        let frame = iced::Rectangle::with_size(iced::Size::new(width, 400.0));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}: {spilled:?}");
        // A squeezed row would leave the poster tile a sliver.
        fn sizes(layout: iced::advanced::Layout<'_>, found: &mut Vec<iced::Size>) {
            found.push(layout.bounds().size());
            for child in layout.children() {
                sizes(child, found);
            }
        }
        let mut found = Vec::new();
        sizes(iced::advanced::Layout::new(&probe.node), &mut found);
        assert!(
            found
                .iter()
                .any(|size| size.width >= 148.0 && size.height >= 222.0),
            "{width}: the poster tile lost its size"
        );
    }
}

/// Picked art adds a Remove button under each tile, which must not push
/// the editor outside a narrow dialog.
#[test]
fn the_editor_with_picks_fits_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let mut draft = Draft::new();
    for kind in Kind::ALL {
        draft.local.insert(
            kind,
            Local {
                path: "/art.png".to_owned(),
                ..Local::default()
            },
        );
    }
    let gutter = crate::flow::heroic::SCROLL_GUTTER;
    for width in [280.0_f32 - 48.0 - gutter, 780.0 - gutter] {
        let mut view: Element<'_, Event> =
            iced::widget::scrollable(draft.view(Palette::DARK, true))
                .height(400)
                .into();
        let probe = probe(&mut view, iced::Size::new(width, 400.0));
        let frame = iced::Rectangle::with_size(iced::Size::new(width, 400.0));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}: {spilled:?}");
    }
}

#[test]
fn stale_generation_skips_decode() {
    let generation = AtomicU64::new(2);
    let result = decode_if_current(&generation, 1, Some(()), |()| -> Result<u8, String> {
        panic!("decoded a stale download")
    });
    assert_eq!(result, Ok(None));
    let result = decode_if_current(&generation, 2, Some(()), |()| Ok(7u8));
    assert_eq!(result, Ok(Some(7)));
}

#[test]
fn clearing_bumps_shared_generation() {
    let mut draft = Draft::new();
    let generation = draft.fetch_generation.clone();
    draft.clear_fetched();
    assert_eq!(generation.load(Ordering::Relaxed), draft.fetch_sequence);
}

#[test]
fn fetch_uses_supplied_availability_without_storing_key_presence() {
    let mut draft = Draft::new();
    draft.kinds = vec![Kind::Icon];
    draft.remote.insert(Kind::Icon, Remote::default());
    let _ = draft.update_with_key_availability(Event::Fetch(true), false);
    assert!(!draft.fetch);
    let _ = draft.update_with_key_availability(Event::Fetch(true), true);
    assert!(draft.fetch);
    assert!(draft.remote.is_empty());
}

#[test]
fn fetch_handles_are_pruned_and_dropped_kinds_are_aborted() {
    let mut draft = Draft::new();
    draft.fetch = true;
    draft.kinds = vec![Kind::Icon, Kind::Hero];
    let (_, icon) = Task::<Event>::none().abortable();
    let (_, hero) = Task::<Event>::none().abortable();
    draft.fetch_handles.insert(Kind::Icon, icon.clone());
    draft.fetch_handles.insert(Kind::Hero, hero.clone());
    draft.configuration_sequence = 1;
    let _ = draft.update_with_key_availability(Event::Configured(1, Ok(ArtFetch::Icon)), true);
    assert!(hero.is_aborted());
    assert!(!icon.is_aborted());
    assert_eq!(
        draft.fetch_handles.keys().copied().collect::<Vec<_>>(),
        vec![Kind::Icon]
    );
    let _ = draft.update(Event::Fetched(Kind::Icon, draft.fetch_sequence, Ok(None)));
    assert!(draft.fetch_handles.is_empty());
}

#[test]
fn typing_title_does_not_discard_pending_art_configuration() {
    let mut draft = Draft::new();
    draft.configuration_sequence = 1;
    let _ = draft.set_title("new title");
    let _ = draft.update(Event::Configured(1, Ok(ArtFetch::default())));
    assert!(!draft.kinds.is_empty());
}

#[test]
fn edited_search_keeps_selected_match_and_preview_when_title_changes() {
    let mut draft = Draft::new();
    draft.search = "Chosen match".to_owned();
    draft.search_edited = true;
    draft.selected = Some(Match(Game {
        id: 7,
        name: "Chosen match".to_owned(),
        year: None,
    }));
    draft.remote.insert(
        Kind::Icon,
        Remote {
            preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
            ..Remote::default()
        },
    );
    let sequence = draft.fetch_sequence;
    let _ = draft.set_title("Renamed environment");
    assert_eq!(draft.selected.as_ref().unwrap().0.id, 7);
    assert!(draft.remote[&Kind::Icon].preview.is_some());
    assert_eq!(draft.fetch_sequence, sequence);
}

#[test]
fn failed_search_keeps_selected_match_and_preview() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = Draft::new();
    draft.paths = Some(paths(root.path()));
    draft.search = "game".to_owned();
    draft.selected = Some(Match(Game {
        id: 7,
        name: "game".to_owned(),
        year: None,
    }));
    draft.remote.insert(
        Kind::Icon,
        Remote {
            preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
            ..Remote::default()
        },
    );
    let sequence = draft.fetch_sequence;
    let _ = draft.update_with_key_availability(Event::Search, true);
    assert!(draft.searching);
    assert_eq!(draft.fetch_sequence, sequence);
    let _ = draft.update(Event::Searched(draft.sequence, Err("offline".to_owned())));
    assert_eq!(draft.selected.as_ref().unwrap().0.id, 7);
    assert!(draft.remote[&Kind::Icon].preview.is_some());
    assert_eq!(draft.fetch_sequence, sequence);
}

#[test]
fn reconfiguring_keeps_loaded_previews_and_fetches_only_added_kinds() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = Draft::new();
    draft.paths = Some(paths(root.path()));
    draft.fetch = true;
    draft.kinds = vec![Kind::Icon];
    draft.selected = Some(Match(Game {
        id: 7,
        name: "game".to_owned(),
        year: None,
    }));
    let image = Arc::new(art::Downloaded {
        asset: 42,
        bytes: vec![1],
        format: art::Format::Png,
    });
    draft.remote.insert(
        Kind::Icon,
        Remote {
            image: Some(image.clone()),
            preview: Some(image::Handle::from_rgba(1, 1, vec![0; 4])),
            ..Remote::default()
        },
    );
    draft.configuration_sequence = 1;
    let sequence = draft.fetch_sequence;
    let _ = draft.update_with_key_availability(Event::Configured(1, Ok(ArtFetch::Icon)), true);
    assert_eq!(draft.remote.len(), 1);
    assert!(Arc::ptr_eq(
        draft.remote[&Kind::Icon].image.as_ref().unwrap(),
        &image
    ));
    assert!(draft.remote[&Kind::Icon].preview.is_some());
    assert_eq!(draft.fetch_sequence, sequence);

    draft.configuration_sequence = 2;
    let _ = draft.update_with_key_availability(
        Event::Configured(2, Ok(ArtFetch::Selected([true, true, false, false]))),
        true,
    );
    assert_eq!(draft.remote.len(), 2);
    assert!(Arc::ptr_eq(
        draft.remote[&Kind::Icon].image.as_ref().unwrap(),
        &image
    ));
    assert!(!draft.remote[&Kind::Icon].loading);
    assert!(draft.remote[&Kind::Grid].loading);
    assert_eq!(draft.fetch_sequence, sequence);
}

fn paths(root: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
}

#[test]
fn local_art_works_without_fetching_and_preserves_source_bytes() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
    let source = root.path().join("my-image.data");
    let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red"/></svg>"#;
    std::fs::write(&source, bytes).unwrap();
    let mut draft = Draft::new();
    draft.local.insert(
        Kind::Icon,
        Local {
            path: source.to_string_lossy().into_owned(),
            ..Local::default()
        },
    );
    assert!(!draft.fetch);
    assert!(draft.kinds.is_empty());
    assert!(draft.validate().is_ok());
    assert!(preview(&source, Kind::Icon).is_ok());
    assert!(draft.apply(&paths, "game").unwrap().is_empty());
    draft.fetch = true;
    draft.kinds = vec![Kind::Icon];
    draft.selected = Some(Match(Game {
        id: 123,
        name: "game".into(),
        year: None,
    }));
    assert!(draft.apply(&paths, "game").unwrap().is_empty());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        std::fs::read(paths.art_dir("game").join("icon.svg")).unwrap(),
        bytes
    );
    assert_eq!(
        art::ArtConfig::load(&paths.art_dir("game")).unwrap().kinds[&Kind::Icon].source,
        art::Source::Manual("my-image.data".to_owned())
    );
}

#[test]
fn stale_previews_and_chooser_results_cannot_replace_an_edit() {
    let mut draft = Draft::new();
    draft.local.insert(
        Kind::Grid,
        Local {
            path: "/new.png".to_owned(),
            sequence: 3,
            chooser_sequence: 3,
            ..Local::default()
        },
    );
    let _ = draft.update(Event::Picked(
        Kind::Grid,
        2,
        Ok(Some(PathBuf::from("/old.png"))),
    ));
    let _ = draft.update(Event::Preview(Kind::Grid, 2, Err("old error".to_owned())));
    assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
    assert!(draft.local[&Kind::Grid].error.is_none());
    let _ = draft.update(Event::Picked(Kind::Grid, 3, Ok(None)));
    assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
    draft.local.get_mut(&Kind::Grid).unwrap().checking = true;
    let _ = draft.update(Event::Browse(Kind::Grid));
    let _ = draft.update(Event::Picked(Kind::Grid, 4, Ok(None)));
    assert!(!draft.manual_entry);
    let _ = draft.update(Event::Browse(Kind::Grid));
    let _ = draft.update(Event::Picked(
        Kind::Grid,
        5,
        Err("portal unavailable".to_owned()),
    ));
    assert!(draft.manual_entry);
    assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
    assert!(draft.local[&Kind::Grid].error.is_none());
    let _ = draft.update(Event::Preview(
        Kind::Grid,
        3,
        Ok(image::Handle::from_rgba(1, 1, vec![0; 4])),
    ));
    assert!(!draft.local[&Kind::Grid].checking);
    assert!(draft.local[&Kind::Grid].preview.is_some());
}

#[test]
fn title_change_refreshes_unedited_search_without_accepting_stale_results() {
    let mut draft = Draft::new();
    draft.search = "Old title".to_owned();
    draft.searching = true;
    draft.sequence = 5;
    let _ = draft.set_title("New title");
    assert_eq!(draft.search, "New title");
    assert!(!draft.searching);
    let _ = draft.update(Event::Searched(
        5,
        Ok(vec![Game {
            id: 1,
            name: "Old title".to_owned(),
            year: None,
        }]),
    ));
    assert!(draft.matches.is_empty());
    let _ = draft.update_with_key_availability(Event::SearchText("Custom match".to_owned()), true);
    let _ = draft.set_title("Third title");
    assert_eq!(draft.search, "Custom match");
}

#[test]
fn failed_art_does_not_remove_registration() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
    let mut draft = Draft::new();
    draft.local.insert(
        Kind::Logo,
        Local {
            path: root
                .path()
                .join("missing.png")
                .to_string_lossy()
                .into_owned(),
            ..Local::default()
        },
    );
    assert!(draft.apply(&paths, "game").is_err());
    assert!(Environment::load(&paths, "game").is_ok());
}

#[test]
fn stale_fetched_art_is_ignored_after_match_change() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = Draft::new();
    draft.paths = Some(paths(root.path()));
    draft.fetch = true;
    draft.kinds = vec![Kind::Grid];
    draft.selected = Some(Match(Game {
        id: 1,
        name: "Old".into(),
        year: None,
    }));
    draft.remote.insert(
        Kind::Grid,
        Remote {
            loading: true,
            ..Remote::default()
        },
    );
    let old_sequence = draft.fetch_sequence;
    let _ = draft.update_with_key_availability(
        Event::Game(Match(Game {
            id: 2,
            name: "New".into(),
            year: None,
        })),
        true,
    );
    let image = Arc::new(art::Downloaded {
        asset: 1,
        bytes: vec![1],
        format: art::Format::Png,
    });
    let handle = image::Handle::from_rgba(1, 1, vec![0; 4]);
    let _ = draft.update(Event::Fetched(
        Kind::Grid,
        old_sequence,
        Ok(Some((image, handle))),
    ));
    assert!(draft.remote[&Kind::Grid].loading);
    assert!(draft.remote[&Kind::Grid].image.is_none());
}

#[test]
fn current_fetched_result_stores_image_and_preview() {
    let mut draft = Draft::new();
    draft.remote.insert(
        Kind::Grid,
        Remote {
            loading: true,
            ..Remote::default()
        },
    );
    let downloaded = Arc::new(art::Downloaded {
        asset: 42,
        bytes: vec![1],
        format: art::Format::Png,
    });
    let preview = image::Handle::from_rgba(1, 1, vec![0; 4]);
    let _ = draft.update(Event::Fetched(
        Kind::Grid,
        draft.fetch_sequence,
        Ok(Some((downloaded.clone(), preview))),
    ));
    assert!(!draft.remote[&Kind::Grid].loading);
    assert!(Arc::ptr_eq(
        draft.remote[&Kind::Grid].image.as_ref().unwrap(),
        &downloaded
    ));
    assert!(draft.remote[&Kind::Grid].preview.is_some());
}

#[test]
fn none_found_preview_skips_fetch_and_reports_note() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
    let mut draft = Draft::new();
    draft.fetch = true;
    draft.kinds = vec![Kind::Grid];
    draft.selected = Some(Match(Game {
        id: 7,
        name: "game".to_owned(),
        year: None,
    }));
    draft.remote.insert(
        Kind::Grid,
        Remote {
            loading: true,
            ..Remote::default()
        },
    );
    let _ = draft.update(Event::Fetched(Kind::Grid, draft.fetch_sequence, Ok(None)));
    assert!(draft.remote[&Kind::Grid].none_found);
    assert!(!draft.remote[&Kind::Grid].loading);
    assert_eq!(
        draft.apply(&paths, "game").unwrap(),
        vec!["No grid artwork found for game"]
    );
}

#[test]
fn local_selection_allows_validation_while_remote_is_loading() {
    let mut draft = Draft::new();
    draft.remote.insert(
        Kind::Grid,
        Remote {
            loading: true,
            ..Remote::default()
        },
    );
    draft.local.insert(
        Kind::Grid,
        Local {
            path: "/grid.png".to_owned(),
            ..Local::default()
        },
    );
    assert!(draft.validate().is_ok());
}

#[test]
fn local_selection_wins_over_fetched_preview() {
    let mut draft = Draft::new();
    let local_preview = image::Handle::from_rgba(1, 1, vec![0; 4]);
    draft.local.insert(
        Kind::Icon,
        Local {
            path: "/icon.png".into(),
            preview: Some(local_preview),
            ..Local::default()
        },
    );
    draft.remote.insert(
        Kind::Icon,
        Remote {
            preview: Some(image::Handle::from_rgba(1, 1, vec![255; 4])),
            ..Remote::default()
        },
    );
    let (shown, fetched) = draft.art_preview(Kind::Icon).unwrap();
    assert!(!fetched);
    assert!(std::ptr::eq(
        shown,
        draft.local[&Kind::Icon].preview.as_ref().unwrap()
    ));
}

fn decoded_size(handle: &image::Handle) -> (u32, u32) {
    match handle {
        image::Handle::Rgba { width, height, .. } => (*width, *height),
        _ => panic!("expected decoded pixels"),
    }
}

#[test]
fn hero_previews_decode_large_enough_for_the_banner() {
    let mut png = Vec::new();
    ::image::RgbaImage::new(1920, 620)
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            ::image::ImageFormat::Png,
        )
        .unwrap();
    assert_eq!(
        decoded_size(&preview_bytes(&png, Kind::Hero).unwrap()),
        (1000, 323)
    );
    let (width, height) = decoded_size(&preview_bytes(&png, Kind::Icon).unwrap());
    assert!(width <= 160 && height <= 100);
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="620"><rect width="1920" height="620" fill="red"/></svg>"#;
    assert_eq!(
        decoded_size(&preview_bytes(svg, Kind::Hero).unwrap()),
        (1000, 323)
    );
    assert_eq!(preview_bounds(Kind::Grid), (POSTER_WIDTH, POSTER_HEIGHT));
}

#[test]
fn banner_statuses_list_hero_logo_and_icon_with_something_to_say() {
    let mut draft = Draft::new();
    assert!(draft.banner_statuses().is_empty());
    draft.fetch = true;
    draft.kinds = Kind::ALL.to_vec();
    draft.remote.insert(
        Kind::Logo,
        Remote {
            loading: true,
            ..Remote::default()
        },
    );
    draft.remote.insert(
        Kind::Icon,
        Remote {
            none_found: true,
            ..Remote::default()
        },
    );
    draft.local.insert(
        Kind::Hero,
        Local {
            path: "/hero.png".to_owned(),
            ..Local::default()
        },
    );
    assert_eq!(
        draft.banner_statuses(),
        vec![
            (Kind::Hero, vec![], true),
            (
                Kind::Logo,
                vec![Note::Info("Fetching...".to_owned())],
                false
            ),
            (
                Kind::Icon,
                vec![Note::Info("No icon artwork found".to_owned())],
                false
            ),
        ]
    );
    assert_eq!(
        draft.notes(Kind::Icon),
        vec![Note::Info("No icon artwork found".to_owned())]
    );
}

#[test]
fn local_selection_hides_fetch_status() {
    let mut draft = Draft::new();
    draft.fetch = true;
    draft.kinds = vec![Kind::Hero];
    draft.remote.insert(
        Kind::Hero,
        Remote {
            error: Some("offline".to_owned()),
            ..Remote::default()
        },
    );
    assert_eq!(draft.notes(Kind::Hero), vec![Note::Error("offline")]);
    draft.local.insert(
        Kind::Hero,
        Local {
            path: "/hero.png".to_owned(),
            checking: true,
            ..Local::default()
        },
    );
    assert_eq!(
        draft.notes(Kind::Hero),
        vec![Note::Info("Checking image...".to_owned())]
    );
}

#[test]
fn apply_stores_held_art_without_a_request() {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
    let mut draft = Draft::new();
    draft.fetch = true;
    draft.kinds = vec![Kind::Grid];
    draft.selected = Some(Match(Game {
        id: 7,
        name: "game".into(),
        year: None,
    }));
    draft.remote.insert(
        Kind::Grid,
        Remote {
            image: Some(Arc::new(art::Downloaded {
                asset: 42,
                bytes: PNG.to_vec(),
                format: art::Format::Png,
            })),
            ..Remote::default()
        },
    );
    assert!(draft.apply(&paths, "game").unwrap().is_empty());
    assert_eq!(
        std::fs::read(paths.art_dir("game").join("grid.png")).unwrap(),
        PNG
    );
    assert_eq!(art::saved_game(&paths, "game").unwrap(), Some(7));
    assert_eq!(
        art::ArtConfig::load(&paths.art_dir("game")).unwrap().kinds[&Kind::Grid].source,
        art::Source::SteamGridDb(42)
    );
}

#[test]
fn a_prefilled_pick_is_stored_under_its_name_until_removed_or_replaced() {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const FETCHED: &[u8] = b"\xff\xd8\xff\xe0 fetched";
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let poster = root.path().join("0123abcd");
    std::fs::write(&poster, PNG).unwrap();
    let mine = root.path().join("mine.png");
    std::fs::write(&mine, PNG).unwrap();
    let fetched = || {
        Draft::with_fetched(
            Kind::Grid,
            7,
            art::Downloaded {
                asset: 42,
                bytes: FETCHED.to_vec(),
                format: art::Format::Jpeg,
            },
        )
    };
    let prefill = |draft: &mut Draft| {
        draft.prefill(
            Kind::Grid,
            &poster,
            "heroic-a.png".to_owned(),
            "From Heroic",
            image::Handle::from_rgba(1, 1, vec![0; 4]),
        )
    };
    let grid = |env: &str| {
        art::ArtConfig::load(&paths.art_dir(env)).unwrap().kinds[&Kind::Grid]
            .source
            .clone()
    };
    for env in ["kept", "removed", "replaced"] {
        Environment::create_native(&paths, &GlobalConfig::default(), env, None, false).unwrap();
    }

    let mut draft = fetched();
    prefill(&mut draft);
    assert!(draft.is_selected(Kind::Grid));
    assert_eq!(
        draft.notes(Kind::Grid),
        [Note::Info("From Heroic".to_owned())]
    );
    draft.apply(&paths, "kept").unwrap();
    assert_eq!(grid("kept"), art::Source::Manual("heroic-a.png".into()));
    assert!(!paths.art_dir("kept").join("grid.jpg").exists());

    let mut draft = fetched();
    prefill(&mut draft);
    let _ = draft.update(Event::Clear(Kind::Grid));
    assert!(!draft.is_selected(Kind::Grid));
    draft.apply(&paths, "removed").unwrap();
    assert_eq!(grid("removed"), art::Source::SteamGridDb(42));

    let mut draft = fetched();
    prefill(&mut draft);
    let _ = draft.update(Event::Path(Kind::Grid, mine.to_string_lossy().into_owned()));
    assert_eq!(draft.prefilled(Kind::Grid), None);
    draft.apply(&paths, "replaced").unwrap();
    assert_eq!(grid("replaced"), art::Source::Manual("mine.png".into()));
}
