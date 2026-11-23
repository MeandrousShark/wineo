use super::*;
use wineo_core::environment::Environment;

/// The editor that Heroic details and Import prefix embed, at the content
/// width of the narrowest and widest import dialogs.
#[test]
fn the_editor_fits_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let draft = Draft::new();
    // Page padding and the scrollbar gutter come off the dialog width.
    let gutter = crate::flow::frame::SCROLL_GUTTER;
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
    let gutter = crate::flow::frame::SCROLL_GUTTER;
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
    Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
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
    assert!(draft.apply(&paths, "game", None).art.unwrap().is_empty());
    draft.fetch = true;
    draft.kinds = vec![Kind::Icon];
    draft.selected = Some(Match(Game {
        id: 123,
        name: "game".into(),
        year: None,
    }));
    assert!(draft.apply(&paths, "game", None).art.unwrap().is_empty());
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
fn a_search_result_from_an_older_search_is_dropped() {
    let mut draft = Draft::new();
    draft.search = "Old title".to_owned();
    draft.searching = true;
    draft.sequence = 5;
    // Editing the search text starts over, so the answer to the old one is stale.
    let _ = draft.update_with_key_availability(Event::SearchText("New title".to_owned()), true);
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
}

#[test]
fn failed_art_does_not_remove_registration() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
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
    assert!(draft.apply(&paths, "game", None).art.is_err());
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
    Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
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
        draft.apply(&paths, "game", None).art.unwrap(),
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
fn notes_list_what_each_kind_has_to_say() {
    let mut draft = Draft::new();
    assert!(Kind::ALL.iter().all(|kind| draft.notes(*kind).is_empty()));
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
    assert_eq!(draft.notes(Kind::Hero), vec![]);
    assert!(draft.is_selected(Kind::Hero));
    assert_eq!(
        draft.notes(Kind::Logo),
        vec![Note::Info("Fetching...".to_owned())]
    );
    assert!(!draft.is_selected(Kind::Logo));
    assert_eq!(
        draft.notes(Kind::Icon),
        vec![Note::Info("No icon artwork found".to_owned())]
    );
    assert!(!draft.is_selected(Kind::Icon));
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

/// A draft with a game chosen and a grid already downloaded for it.
fn draft_with_held_grid() -> Draft {
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
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
                format: art::Format::Png,
            })),
            ..Remote::default()
        },
    );
    draft
}

#[test]
fn apply_stores_nothing_in_an_environment_replaced_since_it_started() {
    for replaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(
            &paths,
            &GlobalConfig::default(),
            "game",
            None,
            false,
            |_| {},
        )
        .unwrap();
        let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
        if replaced {
            wineo_core::test_support::replace_environment(&paths, "game").unwrap();
        }
        let result = draft_with_held_grid()
            .apply(&paths, "game", Some(identity))
            .art;
        let grid = paths.art_dir("game").join("grid.png");
        if replaced {
            assert_eq!(
                result.unwrap_err(),
                FetchError::Changed {
                    env: "game".into(),
                    id: true
                }
            );
            assert!(!grid.exists());
            assert_eq!(art::saved_game(&paths, "game").unwrap(), None);
        } else {
            assert!(result.unwrap().is_empty());
            assert!(grid.exists());
            assert_eq!(art::saved_game(&paths, "game").unwrap(), Some(7));
        }
    }
}

#[test]
fn local_tiles_are_stored_only_in_the_environment_they_started_with() {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    for replaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(
            &paths,
            &GlobalConfig::default(),
            "game",
            None,
            false,
            |_| {},
        )
        .unwrap();
        let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
        let picked = root.path().join("mine.png");
        let poster = root.path().join("poster.png");
        std::fs::write(&picked, PNG).unwrap();
        std::fs::write(&poster, PNG).unwrap();
        let mut draft = Draft::new();
        // A user-picked hero, and a grid the flow prefilled from Heroic.
        draft.local.insert(
            Kind::Hero,
            Local {
                path: picked.display().to_string(),
                ..Local::default()
            },
        );
        draft.local.insert(
            Kind::Grid,
            Local {
                path: poster.display().to_string(),
                prefilled: Some(Prefilled {
                    original: "heroic-game.png".to_owned(),
                    note: "From Heroic",
                }),
                ..Local::default()
            },
        );
        if replaced {
            wineo_core::test_support::replace_environment(&paths, "game").unwrap();
        }
        let result = draft.apply(&paths, "game", Some(identity)).art;
        let art = paths.art_dir("game");
        if replaced {
            assert_eq!(
                result.unwrap_err(),
                FetchError::Changed {
                    env: "game".into(),
                    id: true
                }
            );
            assert!(!art.join("hero.png").exists() && !art.join("grid.png").exists());
        } else {
            assert!(result.unwrap().is_empty());
            assert!(art.join("hero.png").exists() && art.join("grid.png").exists());
        }
    }
}

#[test]
fn held_art_is_published_only_into_the_environment_it_was_read_from() {
    for replaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(
            &paths,
            &GlobalConfig::default(),
            "game",
            None,
            false,
            |_| {},
        )
        .unwrap();
        let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
        // Past the game record, which `apply` saves first under the same
        // check, so this is the publish's own.
        art::save_game(&paths, "game", 7).unwrap();
        if replaced {
            wineo_core::test_support::replace_environment(&paths, "game").unwrap();
        }
        let draft = draft_with_held_grid();
        let game = Game {
            id: 7,
            name: "game".into(),
            year: None,
        };
        let (mut notes, mut failures, mut icon) = (Vec::new(), Vec::new(), false);
        let missing = draft.publish_held(
            &paths,
            "game",
            &game,
            &[Kind::Grid],
            identity,
            (&mut notes, &mut failures, &mut icon),
        );
        assert!(missing.is_empty() && notes.is_empty() && !icon);
        let grid = paths.art_dir("game").join("grid.png");
        if replaced {
            assert_eq!(
                failures,
                [FetchError::Changed {
                    env: "game".into(),
                    id: true
                }]
            );
            assert!(!grid.exists());
        } else {
            assert!(failures.is_empty());
            assert!(grid.exists());
        }
    }
}

#[test]
fn apply_stores_held_art_without_a_request() {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "game",
        None,
        false,
        |_| {},
    )
    .unwrap();
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
    assert!(draft.apply(&paths, "game", None).art.unwrap().is_empty());
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
        Environment::create_native(&paths, &GlobalConfig::default(), env, None, false, |_| {})
            .unwrap();
    }

    let mut draft = fetched();
    prefill(&mut draft);
    assert!(draft.is_selected(Kind::Grid));
    assert_eq!(
        draft.notes(Kind::Grid),
        [Note::Info("From Heroic".to_owned())]
    );
    draft.apply(&paths, "kept", None).art.unwrap();
    assert_eq!(grid("kept"), art::Source::Manual("heroic-a.png".into()));
    assert!(!paths.art_dir("kept").join("grid.jpg").exists());

    let mut draft = fetched();
    prefill(&mut draft);
    let _ = draft.update(Event::Clear(Kind::Grid));
    assert!(!draft.is_selected(Kind::Grid));
    draft.apply(&paths, "removed", None).art.unwrap();
    assert_eq!(grid("removed"), art::Source::SteamGridDb(42));

    let mut draft = fetched();
    prefill(&mut draft);
    let _ = draft.update(Event::Path(Kind::Grid, mine.to_string_lossy().into_owned()));
    assert_eq!(draft.prefilled(Kind::Grid), None);
    draft.apply(&paths, "replaced", None).art.unwrap();
    assert_eq!(grid("replaced"), art::Source::Manual("mine.png".into()));
}

#[test]
fn the_fetch_and_the_entry_are_told_apart() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let notes = vec!["No logo artwork found".to_owned()];
    let answer = notes.clone();
    let fetcher: Fetcher =
        Arc::new(move |_, _, _, identity, _| Fetched::answer(Ok(answer.clone()), identity));
    let parts = |parts: FetchParts| (parts.art, parts.entries);
    // Nothing to refresh for an environment that isn't there.
    assert_eq!(
        parts(fetch_art_parts(
            &paths,
            "game",
            "Game",
            None,
            &Token::new(),
            &fetcher
        )),
        (Ok(notes), Ok(()))
    );
    let failing: Fetcher = Arc::new(|_, _, _, identity, _| {
        Fetched::answer(Err(FetchError::Other("offline".to_owned())), identity)
    });
    let offline = Err(FetchError::Other("offline".to_owned()));
    assert_eq!(
        parts(fetch_art_parts(
            &paths,
            "game",
            "Game",
            None,
            &Token::new(),
            &failing
        )),
        (offline.clone(), Ok(()))
    );
    // A skipped fetch leaves the entry alone and reports it as fine.
    let token = Token::new();
    token.cancel();
    assert_eq!(
        parts(fetch_art_parts(
            &paths, "game", "Game", None, &token, &failing
        )),
        (offline, Ok(()))
    );
}

#[test]
fn a_replacement_environments_entries_are_left_alone_after_a_fetch() {
    for replaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let exe = root.path().join("notepad");
        crate::test_script::write_script(
            &exe,
            "#!/bin/sh
exit 0
",
        );
        let config = paths.environment_config("game");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "schema_version = 1
kind = 'native'
[targets.alt]
exe = '{}'
desktop = true
",
                exe.display()
            ),
        )
        .unwrap();
        // The environment is replaced while the fetch runs.
        let fetcher: Fetcher = Arc::new(move |paths, name, _, identity, _| {
            if replaced {
                wineo_core::test_support::replace_environment(paths, name).unwrap();
            }
            Fetched::answer(Ok(Vec::new()), identity)
        });
        let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
        let parts = fetch_art_parts(
            &paths,
            "game",
            "Game",
            Some(identity),
            &Token::new(),
            &fetcher,
        );
        assert_eq!((parts.art, parts.entries), (Ok(Vec::new()), Ok(())));
        let written = paths
            .applications_dir()
            .join("wineo-game-alt.desktop")
            .exists();
        assert_eq!(written, !replaced);
    }
}

#[test]
fn several_kinds_failing_with_one_changed_among_them_is_changed() {
    let other = |line: &str| FetchError::Other(line.to_owned());
    let unreadable = FetchError::LibraryUnreadable("library.toml can't be read.".to_owned());
    for changed in [
        FetchError::Changed {
            env: "game".into(),
            id: true,
        },
        FetchError::Changed {
            env: "game".into(),
            id: false,
        },
    ] {
        assert_eq!(
            FetchError::combine(vec![other("a"), changed.clone(), unreadable.clone()]),
            Some(changed)
        );
    }
    // The library's sentence once, ahead of the other kinds' lines.
    assert_eq!(
        FetchError::combine(vec![other("a"), unreadable.clone(), other("b"), unreadable]),
        Some(FetchError::LibraryUnreadable(
            "library.toml can't be read.\na\nb".to_owned()
        ))
    );
    assert_eq!(
        FetchError::combine(vec![other("a"), other("b")]),
        Some(other("a\nb"))
    );
    assert_eq!(FetchError::combine(Vec::new()), None);
}

#[test]
fn a_changed_failure_says_replaced_for_an_id_and_changed_for_a_fingerprint() {
    let changed = |id| FetchError::Changed {
        env: "hades".into(),
        id,
    };
    assert_eq!(
        changed(true).to_string(),
        "`hades` was replaced. Artwork wasn't stored."
    );
    assert_eq!(
        changed(false).to_string(),
        "`hades` changed. Artwork wasn't stored."
    );
    assert_eq!(
        changed_message("hades", true, NOT_REMOVED),
        "`hades` was replaced. Artwork wasn't removed."
    );
    // Already a sentence about the environment, so no "Artwork: " in front.
    assert_eq!(changed(true).note(), changed(true).to_string());
    assert_eq!(
        FetchError::Other("offline".into()).note(),
        "Artwork: offline"
    );
}

/// A native environment from before ids: a config and no library entry.
fn old_environment(paths: &Paths) {
    let config = paths.environment_config("game");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();
}

#[test]
fn a_draft_with_nothing_to_write_gives_no_id() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    old_environment(&paths);
    // Empty, as a Heroic import with no poster and fetching off leaves it,
    // and with fetching on but no match chosen.
    let mut draft = Draft::new();
    let applied = draft.apply(&paths, "game", None);
    assert_eq!(applied.art, Ok(Vec::new()));
    assert_eq!(applied.identity, None);
    draft.fetch = true;
    draft.kinds = vec![Kind::Grid];
    assert_eq!(draft.apply(&paths, "game", None).art, Ok(Vec::new()));
    assert!(!paths.library_file().exists());

    // Something to write takes one.
    let applied = draft_with_held_grid().apply(&paths, "game", None);
    assert_eq!(applied.art, Ok(Vec::new()));
    let id = wineo_core::library::load(&paths).environments["game"].id;
    assert!(id.is_some());
    assert_eq!(applied.identity.and_then(|identity| identity.id()), id);
}

/// A native environment with a desktop target, so `desktop::apply` writes
/// an entry for it, under `paths`.
fn desktop_environment(root: &Path, paths: &Paths) -> PathBuf {
    let exe = root.join("notepad");
    crate::test_script::write_script(&exe, "#!/bin/sh\nexit 0\n");
    let config = paths.environment_config("game");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "schema_version = 1\nkind = 'native'\n[targets.alt]\nexe = '{}'\ndesktop = true\n",
            exe.display()
        ),
    )
    .unwrap();
    paths.applications_dir().join("wineo-game-alt.desktop")
}

#[test]
fn a_changed_fetch_leaves_the_desktop_entries_alone() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let entry = desktop_environment(root.path(), &paths);
    let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
    // Changed, though the environment is still there: the fetch's word is
    // what counts, since the check that decided ran under the art lock.
    let fetcher: Fetcher = Arc::new(|_, _, _, identity, _| {
        Fetched::answer(
            Err(FetchError::Changed {
                env: "game".into(),
                id: true,
            }),
            identity,
        )
    });
    let parts = fetch_art_parts(
        &paths,
        "game",
        "Game",
        Some(identity),
        &Token::new(),
        &fetcher,
    );
    assert_eq!(
        parts.art,
        Err(FetchError::Changed {
            env: "game".into(),
            id: true
        })
    );
    assert_eq!(parts.entries, Ok(()));
    assert!(!entry.exists());

    // Any other failure still brings the entries in line.
    let fetcher: Fetcher = Arc::new(|_, _, _, identity, _| {
        Fetched::answer(Err(FetchError::Other("offline".to_owned())), identity)
    });
    fetch_art_parts(
        &paths,
        "game",
        "Game",
        Some(identity),
        &Token::new(),
        &fetcher,
    );
    assert!(entry.exists());
}

#[test]
fn entries_skipped_for_an_unreadable_library_are_said_only_off_a_portable_root() {
    for (portable, icon, said) in [
        (false, true, true),
        (false, false, false),
        (true, true, false),
    ] {
        let case = format!("portable {portable}, icon {icon}");
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let paths = Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            portable.then(|| root.path().join("portable")).as_deref(),
        )
        .unwrap();
        let entry = desktop_environment(root.path(), &paths);
        let identity = EnvironmentIdentity::acquire_existing(&paths, "game").unwrap();
        assert!(identity.id().is_some(), "{case}");
        // The library breaks after the icon was stored.
        let fetcher: Fetcher = Arc::new(move |paths, _, _, identity, _| {
            std::fs::write(paths.library_file(), "[broken\n").unwrap();
            Fetched {
                art: Ok(Vec::new()),
                identity,
                icon,
            }
        });
        let parts = fetch_art_parts(
            &paths,
            "game",
            "Game",
            Some(identity),
            &Token::new(),
            &fetcher,
        );
        assert_eq!(parts.art, Ok(Vec::new()), "{case}");
        let expected = said.then(|| {
            EntriesError::LibraryUnreadable(
                "library.toml can't be read. The desktop entries for `game` weren't updated."
                    .to_owned(),
            )
        });
        assert_eq!(parts.entries.err(), expected, "{case}");
        assert!(!entry.exists(), "{case}: the entries were left alone");
    }
}

#[test]
fn an_unreadable_library_after_the_fetch_is_one_message_with_the_entries() {
    let error = FetchError::LibraryUnreadable("art sentence".to_owned());
    let entries = EntriesError::LibraryUnreadable("entries sentence".to_owned());
    assert_eq!(
        error.with_entries(&entries),
        FetchError::LibraryUnreadable("art sentence\nentries sentence".to_owned())
    );
    assert_eq!(
        FetchError::Other("offline".to_owned())
            .with_entries(&EntriesError::Failed("no folder".to_owned())),
        FetchError::Other("offline; desktop entry: no folder".to_owned())
    );
}

#[test]
fn any_failure_in_an_environment_replaced_since_is_changed_and_leaves_its_entries() {
    for id in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let entry = desktop_environment(root.path(), &paths);
        let identity = if id {
            EnvironmentIdentity::acquire_existing(&paths, "game").unwrap()
        } else {
            EnvironmentIdentity::peek(&paths, "game").unwrap()
        };
        assert_eq!(identity.id().is_some(), id);
        // Offline, and replaced meanwhile.
        let fetcher: Fetcher = Arc::new(|paths, name, _, identity, _| {
            wineo_core::test_support::replace_environment(paths, name).unwrap();
            Fetched::answer(Err(FetchError::Other("offline".to_owned())), identity)
        });
        let parts = fetch_art_parts(
            &paths,
            "game",
            "Game",
            Some(identity),
            &Token::new(),
            &fetcher,
        );
        assert_eq!(
            parts.art,
            Err(FetchError::Changed {
                env: "game".into(),
                id
            }),
            "id {id}"
        );
        assert_eq!(parts.entries, Ok(()));
        assert!(
            !entry.exists(),
            "id {id}: the replacement's entry was written"
        );
    }
}

/// A legacy `library.toml` with `game` in it and no id, whose bytes a
/// write that writes nothing must leave alone.
fn legacy_library(paths: &Paths) -> Vec<u8> {
    let bytes = b"schema_version = 1\n\n[environments.game]\nfavorite = true\n".to_vec();
    std::fs::write(paths.library_file(), &bytes).unwrap();
    bytes
}

#[test]
fn a_default_fetch_that_writes_nothing_gives_an_old_environment_no_id() {
    use crate::flow::artwork::canned::canned_hades;
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    old_environment(&paths);
    let before = legacy_library(&paths);
    let fetch = |title: &str, connected: bool, token: &Token| {
        let answers = canned_hades();
        fetch_default_over(&paths, "game", title, None, token, &move || {
            if connected {
                Ok((Box::new(answers.clone()), "key".to_owned()))
            } else {
                Err("no SteamGridDB key".to_owned())
            }
        })
    };
    // No key, a search that fails, no match, and a cancel.
    for (title, connected) in [("Hades", false), ("Nothing at all", true)] {
        let fetched = fetch(title, connected, &Token::new());
        assert!(fetched.art.is_err(), "{title}: {:?}", fetched.art);
        assert_eq!(fetched.identity, None, "{title}");
    }
    let mut no_match = canned_hades();
    std::sync::Arc::make_mut(&mut no_match.0).insert(
        format!("{}/search/autocomplete/Hades", wineo_core::steamgriddb::API),
        (200, br#"{"success":true,"data":[]}"#.to_vec()),
    );
    let fetched = fetch_default_over(&paths, "game", "Hades", None, &Token::new(), &move || {
        Ok((Box::new(no_match.clone()), "key".to_owned()))
    });
    assert_eq!(
        fetched.art,
        Ok(vec!["No artwork match for Hades".to_owned()])
    );
    let token = Token::new();
    let canceling = canned_hades().on("/search/", {
        let token = token.clone();
        move || token.cancel()
    });
    let fetched = fetch_default_over(&paths, "game", "Hades", None, &token, &move || {
        Ok((Box::new(canceling.clone()), "key".to_owned()))
    });
    assert!(fetched.art.is_err());
    assert_eq!(std::fs::read(paths.library_file()).unwrap(), before);

    // A match it can write to takes the id.
    let fetched = fetch("Hades", true, &Token::new());
    assert!(fetched.art.is_ok(), "{:?}", fetched.art);
    let id = wineo_core::library::load(&paths).environments["game"].id;
    assert!(id.is_some());
    assert_eq!(fetched.identity.and_then(|identity| identity.id()), id);
}

#[test]
fn an_old_environment_edited_during_the_search_is_refused_as_changed() {
    use crate::flow::artwork::canned::canned_hades;
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    old_environment(&paths);
    let before = legacy_library(&paths);
    let editing = canned_hades().on("/search/", {
        let paths = paths.clone();
        move || {
            let config = paths.environment_config("game");
            let text = std::fs::read_to_string(&config).unwrap();
            let staged = config.with_extension("saving");
            std::fs::write(&staged, format!("# edited\n{text}")).unwrap();
            std::fs::rename(staged, config).unwrap();
        }
    });
    let fetched = fetch_default_over(&paths, "game", "Hades", None, &Token::new(), &move || {
        Ok((Box::new(editing.clone()), "key".to_owned()))
    });
    assert_eq!(
        fetched.art,
        Err(FetchError::Changed {
            env: "game".into(),
            id: false
        })
    );
    assert_eq!(std::fs::read(paths.library_file()).unwrap(), before);
}

#[test]
fn a_draft_whose_files_cant_be_stored_gives_no_id() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    old_environment(&paths);
    let before = legacy_library(&paths);
    let notes = root.path().join("notes.png");
    std::fs::write(&notes, b"just some notes\n").unwrap();
    let mut draft = Draft::new();
    for (kind, path) in [
        (Kind::Grid, notes.clone()),
        (Kind::Hero, root.path().join("missing.png")),
    ] {
        draft.local.insert(
            kind,
            Local {
                path: path.display().to_string(),
                ..Local::default()
            },
        );
    }
    let applied = draft.apply(&paths, "game", None);
    assert!(applied.art.is_err());
    assert_eq!(applied.identity, None);
    assert_eq!(std::fs::read(paths.library_file()).unwrap(), before);
}
