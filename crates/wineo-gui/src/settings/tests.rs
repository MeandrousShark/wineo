use crate::test_script::write_script;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use wineo_core::config::GlobalConfig;
use wineo_core::settings::{self, ConfigFile};

use super::*;
use form::{EnvRow, Source};
use handle::refused;
use load::Current;

fn global_page(paths: &Paths) -> Page {
    let mut page = Page::new_global(1);
    let _ = page.reload(paths, Mode::Replace);
    let _ = page.update(paths, Event::Loaded(1, load::snapshot_global(paths)));
    page
}

fn global_editor(page: &Page) -> &global::Editor {
    match &page.view {
        View::Global(editor) => editor,
        _ => panic!("global form expected"),
    }
}

#[test]
fn steamgriddb_key_work_does_not_block_unrelated_saves() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
    assert!(page.is_dirty());

    for event in [
        Event::SteamGridDbKeySetOpen,
        Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
        Event::SteamGridDbKeySet,
    ] {
        assert!(!event.edits());
        let _ = page.update(&paths, event);
    }
    assert!(!page.navigation_blocked());
    assert!(page.can_save());
    assert!(page.steamgriddb.input.as_str().is_empty());
    let _ = page.update(&paths, Event::Save);
    assert!(page.saving);
    let input_session = page.steamgriddb_input_session;
    let sequence = page.steamgriddb_sequence;
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeySaved(
            page.generation,
            sequence,
            input_session,
            Err("SteamGridDB rejected the key".to_owned()),
        ),
    );

    assert!(page.is_dirty());
    assert_eq!(page.changes(), 1);
    assert!(!page.leave_prompt);
    assert!(page.navigation_blocked());
    assert!(page.steamgriddb.input_open);
    assert!(page.steamgriddb.input.as_str().is_empty());
    assert_eq!(
        page.steamgriddb.error.as_deref(),
        Some("SteamGridDB rejected the key")
    );
}

#[test]
fn steamgriddb_key_save_and_remove_results_stay_outside_the_save_bar() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::SteamGridDbKeySetOpen);
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
    );
    let _ = page.update(&paths, Event::SteamGridDbKeySet);
    let input_session = page.steamgriddb_input_session;
    let sequence = page.steamgriddb_sequence;
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeySaved(
            page.generation,
            sequence,
            input_session,
            Ok(steamgriddb::Saved {
                status: steamgriddb::Status {
                    source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                    override_note: false,
                },
                warning: None,
                result: Some("Saved the SteamGridDB key to the keyring".to_owned()),
            }),
        ),
    );
    let status_sequence = page.steamgriddb_sequence;
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeyStatusLoaded(
            page.generation,
            status_sequence,
            Ok(steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            }),
        ),
    );
    assert!(!page.is_dirty());
    assert_eq!(page.changes(), 0);
    assert!(!page.leave_prompt);
    assert_eq!(
        page.steamgriddb.status.as_ref().unwrap().label(&paths),
        "Saved in the keyring"
    );

    let _ = page.update(&paths, Event::SteamGridDbKeyRemove);
    let input_session = page.steamgriddb_input_session;
    let sequence = page.steamgriddb_sequence;
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeyRemoved(
            page.generation,
            sequence,
            input_session,
            Ok(steamgriddb::Removed {
                status: steamgriddb::Status {
                    source: None,
                    override_note: false,
                },
                result: "Removed the SteamGridDB key".to_owned(),
            }),
        ),
    );
    assert!(!page.is_dirty());
    assert!(!page.leave_prompt);
    assert_eq!(
        page.steamgriddb.result.as_deref(),
        Some("Removed the SteamGridDB key")
    );
}

fn steam_texts(page: &Page, paths: &Paths) -> Vec<String> {
    view::steam_key_row(&page.steam_key, paths).texts()
}

#[test]
fn key_rows_show_only_the_button_that_applies() {
    use wineo_core::key_store::KeySource;
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let buttons = |texts: Vec<String>| -> Vec<String> {
        texts
            .into_iter()
            .filter(|text| text == "Set key..." || text == "Remove key")
            .collect()
    };
    for (source, shown) in [
        (None, vec!["Set key..."]),
        (Some(KeySource::Keyring), vec!["Remove key"]),
        (Some(KeySource::File), vec!["Remove key"]),
        // The variable's key may hide a saved one.
        (Some(KeySource::Variable), vec!["Set key...", "Remove key"]),
    ] {
        page.steamgriddb.status = Some(steamgriddb::Status {
            source,
            override_note: source == Some(KeySource::Variable),
        });
        page.steam_key.status = Some(steam_key::Status {
            source,
            override_note: source == Some(KeySource::Variable),
        });
        let griddb = view::steamgriddb_row(&page.steamgriddb, &paths).texts();
        assert_eq!(buttons(griddb), shown, "{source:?}");
        assert_eq!(buttons(steam_texts(&page, &paths)), shown, "{source:?}");
    }
    // Still checking: both, so neither is out of reach.
    page.steamgriddb.status = None;
    let griddb = view::steamgriddb_row(&page.steamgriddb, &paths).texts();
    assert_eq!(buttons(griddb), ["Set key...", "Remove key"]);
}

#[test]
fn steam_key_row_saves_checks_and_removes_without_showing_the_key() {
    let key = steam_key::tests::KEY;
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let shown = steam_texts(&page, &paths);
    assert_eq!(shown[1], "Checking...");
    assert!(
        shown
            .iter()
            .any(|text| text == "Get a key at https://steamcommunity.com/dev/apikey.")
    );
    let status = page.steam_key_sequence;
    let _ = page.update(
        &paths,
        Event::SteamKeyStatusLoaded(
            page.generation,
            status,
            Ok(steam_key::Status {
                source: None,
                override_note: false,
            }),
        ),
    );
    assert_eq!(steam_texts(&page, &paths)[1], "No key");

    let _ = page.update(&paths, Event::SteamKeySetOpen);
    let _ = page.update(
        &paths,
        Event::SteamKeyText(steamgriddb::Secret::new(key.to_owned())),
    );
    assert!(
        !steam_texts(&page, &paths)
            .iter()
            .any(|text| text.contains(key))
    );
    let _ = page.update(&paths, Event::SteamKeySet);
    assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Saving));
    assert_eq!(page.steam_key.input.as_str(), "");
    assert!(
        steam_texts(&page, &paths)
            .iter()
            .any(|text| text == "Checking and saving...")
    );

    // A refused key leaves the input open and empty, with Steam's reason.
    let sequence = page.steam_key_sequence;
    let session = page.steam_key_input_session;
    let _ = page.update(
        &paths,
        Event::SteamKeySaved(
            page.generation,
            sequence,
            session,
            Err("Couldn't check the key: Steam rejected the Web API key".to_owned()),
        ),
    );
    assert!(page.steam_key.busy.is_none());
    assert!(page.steam_key.input_open);
    assert!(
        steam_texts(&page, &paths)
            .iter()
            .any(|text| text == "Couldn't check the key: Steam rejected the Web API key")
    );

    let _ = page.update(
        &paths,
        Event::SteamKeyText(steamgriddb::Secret::new(key.to_owned())),
    );
    let _ = page.update(&paths, Event::SteamKeySet);
    let sequence = page.steam_key_sequence;
    let session = page.steam_key_input_session;
    let _ = page.update(
        &paths,
        Event::SteamKeySaved(
            page.generation,
            sequence,
            session,
            Ok(steam_key::Saved {
                status: steam_key::Status {
                    source: Some(wineo_core::key_store::KeySource::Keyring),
                    override_note: false,
                },
                warning: None,
                result: Some("Saved the Steam Web API key to the keyring".to_owned()),
            }),
        ),
    );
    assert!(!page.steam_key.input_open);
    let shown = steam_texts(&page, &paths);
    assert_eq!(shown[1], "Saved in the keyring");
    assert!(
        shown
            .iter()
            .any(|text| text == "Saved the Steam Web API key to the keyring")
    );
    assert!(!shown.iter().any(|text| text.contains(key)));
    assert!(!format!("{:?}", page.steam_key).contains(key));
    // The save asked for the status again; an older answer can't land.
    let _ = page.update(
        &paths,
        Event::SteamKeyStatusLoaded(
            page.generation,
            status,
            Ok(steam_key::Status {
                source: None,
                override_note: false,
            }),
        ),
    );
    assert_eq!(steam_texts(&page, &paths)[1], "Saved in the keyring");

    let _ = page.update(&paths, Event::SteamKeyRemove);
    assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Removing));
    let sequence = page.steam_key_sequence;
    let _ = page.update(
        &paths,
        Event::SteamKeyRemoved(
            page.generation,
            sequence,
            None,
            Ok(steam_key::Removed {
                status: steam_key::Status {
                    source: None,
                    override_note: false,
                },
                result: "Removed the Steam Web API key".to_owned(),
            }),
        ),
    );
    let shown = steam_texts(&page, &paths);
    assert_eq!(shown[1], "No key");
    assert!(
        shown
            .iter()
            .any(|text| text == "Removed the Steam Web API key")
    );
    // The SteamGridDB row was never touched.
    assert!(page.steamgriddb.result.is_none());
    assert!(page.steamgriddb.busy.is_none());
}

#[test]
fn steam_key_results_for_a_closed_page_are_dropped() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::SteamKeyRemove);
    let sequence = page.steam_key_sequence;
    let _ = page.update(
        &paths,
        Event::SteamKeyRemoved(
            page.generation + 1,
            sequence,
            None,
            Ok(steam_key::Removed {
                status: steam_key::Status {
                    source: None,
                    override_note: false,
                },
                result: "Removed the Steam Web API key".to_owned(),
            }),
        ),
    );
    assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Removing));
    assert!(page.steam_key.result.is_none());
}

#[test]
fn steam_key_reveal_is_used_once() {
    let (_root, paths) = setup();
    let mut page = Page::new_global(1);
    page.reveal_steam_key = true;
    let _ = page.reload(&paths, Mode::Replace);
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.reveal_steam_key);
}

#[test]
fn steamgriddb_work_does_not_block_reverting_unrelated_edits() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
    let _ = page.update(&paths, Event::SteamGridDbKeySetOpen);
    let _ = page.update(
        &paths,
        Event::SteamGridDbKeyText(steamgriddb::Secret::new("private-value".to_owned())),
    );
    let _ = page.update(&paths, Event::SteamGridDbKeySet);

    assert!(page.is_dirty());
    assert_eq!(page.steamgriddb.busy, Some(steamgriddb::Busy::Saving));
    let _ = page.update(&paths, Event::Revert);

    assert!(!page.is_dirty());
    assert_eq!(page.steamgriddb.busy, Some(steamgriddb::Busy::Saving));
    assert!(!page.navigation_blocked());
}

#[test]
fn global_art_switches_save_every_combination() {
    use wineo_core::art::Kind;
    for mask in 0..16 {
        let (_root, paths) = setup();
        let mut page = global_page(&paths);
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let _ = page.update(&paths, Event::GlobalArtKind(kind, mask & (1 << index) != 0));
        }
        let expected: Vec<_> = Kind::ALL
            .into_iter()
            .enumerate()
            .filter_map(|(index, kind)| (mask & (1 << index) != 0).then_some(kind))
            .collect();
        let edits = global_editor(&page).form.edits();
        load::save_global(&paths, &edits).unwrap();
        assert_eq!(
            GlobalConfig::load(&paths).unwrap().art.fetch.kinds(),
            expected
        );
        assert_eq!(global_editor(&page).form.draft.art_fetch.kinds(), expected);
    }
}

#[test]
fn missing_global_file_shows_defaults_and_is_created_on_save() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert!(!page.has_file());
    assert!(!page.is_dirty());
    assert_eq!(global_editor(&page).form.draft.arch, Arch::Win64);
    let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
    let edits = global_editor(&page).form.edits();
    let report = load::save_global(&paths, &edits).unwrap();
    assert_eq!(report.failed, None);
    assert_eq!(GlobalConfig::load(&paths).unwrap().gamemode, Some(true));
    let _ = page.update(&paths, Event::Save);
    let _ = page.update(&paths, Event::Saved(1, Ok(report)));
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(page.has_file());
    assert!(!page.is_dirty());
}

#[test]
fn the_games_folder_and_gog_language_save_and_validate() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::GogLanguage, "english".into()),
    );
    assert_eq!(
        global_editor(&page).form.problem().as_deref(),
        Some("Invalid gog.language: use a locale tag like en-US.")
    );
    assert!(!page.can_save());
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::GogLanguage, "fr-FR".into()),
    );
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::GamesRoot, "games".into()),
    );
    assert_eq!(
        global_editor(&page).form.problem().as_deref(),
        Some("The path must be absolute or start with ~/.")
    );
    // A folder picked while the field still holds what it held then
    // replaces it; a late one after typing doesn't.
    let _ = page.update(
        &paths,
        Event::PickedGamesRoot(1, "games".into(), Ok(Some("/mnt/games".into()))),
    );
    let _ = page.update(
        &paths,
        Event::PickedGamesRoot(1, "other".into(), Ok(Some("/mnt/late".into()))),
    );
    assert_eq!(global_editor(&page).form.draft.games_root, "/mnt/games");
    assert!(global_editor(&page).form.problem().is_none());
    for size in [iced::Size::new(900.0, 700.0), iced::Size::new(360.0, 280.0)] {
        let mut view = page.view(&paths, crate::theme::Palette::DARK);
        let _ = crate::layout_probe::layout(&mut view, size);
    }
    let edits = global_editor(&page).form.edits();
    assert_eq!(
        edits.pairs,
        [
            ("defaults.games_root".to_owned(), "/mnt/games".to_owned()),
            ("gog.language".to_owned(), "fr-FR".to_owned()),
        ]
    );
    let report = load::save_global(&paths, &edits).unwrap();
    assert_eq!(report.failed, None);
    let saved = GlobalConfig::load(&paths).unwrap();
    assert_eq!(saved.gog_language(), "fr-FR");
    assert_eq!(
        saved.games_root(&paths).unwrap(),
        std::path::PathBuf::from("/mnt/games")
    );
    let page = global_page(&paths);
    assert_eq!(global_editor(&page).form.draft.gog_language, "fr-FR");
    assert!(!page.is_dirty());
}

#[test]
fn a_portable_page_leaves_the_gog_install_fields_alone() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| std::ffi::OsString::from(&home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    let mut page = global_page(&paths);
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::GogLanguage, "fr-FR".into()),
    );
    let _ = page.update(&paths, Event::BrowseGamesRoot);
    assert!(!page.is_dirty());
}

#[test]
fn broken_or_unsupported_global_file_shows_error_and_reloads() {
    let (_root, paths) = setup();
    write_global(&paths, "stop_timeout = [\n");
    let mut page = global_page(&paths);
    assert!(matches!(page.view, View::Error(_)));
    assert!(page.has_file());
    write_global(&paths, "schema_version = 999\n");
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    assert!(page.loading());
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(matches!(page.view, View::Error(_)));
    write_global(&paths, "gamemode = true\n");
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert_eq!(global_editor(&page).form.draft.gamemode, Some(true));
}

#[test]
fn global_page_checks_only_the_global_file_and_reloads_after_an_edit() {
    let (_root, paths) = setup();
    write_global(&paths, "gamemode = false\n");
    let mut page = global_page(&paths);
    write(&paths, GAME);
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    assert!(!page.loading());
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::LogLimitMb, "75".into()),
    );
    write_global(&paths, "gamemode = true\n");
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    assert!(page.changed_on_disk);
    assert!(!page.can_save() && page.can_reload());
    let _ = page.update(&paths, Event::Reload);
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.is_dirty());
    assert_eq!(global_editor(&page).form.draft.gamemode, Some(true));
}

#[test]
fn global_controls_stay_locked_through_the_save_reload() {
    let (_root, paths) = setup();
    write_global(&paths, "[env]\nA = \"1\"\n");
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalSwitch(Switch::Gamemode, true));
    let edits = global_editor(&page).form.edits();
    let _ = page.update(&paths, Event::Save);
    assert!(page.locked());
    let events = [
        Event::GlobalSwitch(Switch::Wayland, true),
        Event::GlobalRunner(RunnerChoice {
            name: "other".into(),
            launcher: None,
            proton: None,
        }),
        Event::GlobalArch(Arch::Win32),
        Event::GlobalField(global::Field::PrefixRoot, "/tmp/prefixes".into()),
        Event::GlobalField(global::Field::RunnerRoot, "/tmp/runners".into()),
        Event::GlobalField(global::Field::DxvkRoot, "/tmp/dxvk".into()),
        Event::GlobalField(global::Field::Vkd3dRoot, "/tmp/vkd3d".into()),
        Event::GlobalField(global::Field::StopTimeout, "15".into()),
        Event::GlobalField(global::Field::LogLimitMb, "80".into()),
        Event::GlobalArt(ArtFetch::Icon),
        Event::EnvKey(0, "B".into()),
        Event::EnvValue(0, "2".into()),
        Event::EnvRemove(0),
        Event::NewEnvKey("C".into()),
        Event::NewEnvValue("3".into()),
        Event::EnvAdd,
    ];
    for event in events.iter().cloned() {
        let _ = page.update(&paths, event);
    }
    assert_eq!(global_editor(&page).form.edits(), edits);
    assert_eq!(global_editor(&page).new_env, EnvRow::default());
    let report = load::save_global(&paths, &edits).unwrap();
    let _ = page.update(&paths, Event::Saved(1, Ok(report)));
    assert!(page.locked());
    for event in events {
        let _ = page.update(&paths, event);
    }
    assert_eq!(global_editor(&page).form.edits(), edits);
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert!(!page.locked() && !page.is_dirty());
}

#[test]
fn global_pending_env_text_survives_a_quiet_reload_and_revert_clears_it() {
    let (_root, paths) = setup();
    write_global(&paths, "gamemode = false\n");
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::NewEnvKey("NEW".into()));
    let _ = page.update(&paths, Event::NewEnvValue("value".into()));
    write_global(&paths, "gamemode = true\n");
    let sequence = start_check(&mut page);
    let _ = page.update(
        &paths,
        Event::Checked(1, sequence, Texts::read_global(&paths)),
    );
    let _ = page.update(&paths, Event::Loaded(1, load::snapshot_global(&paths)));
    assert_eq!(global_editor(&page).new_env.key, "NEW");
    assert_eq!(global_editor(&page).new_env.value, "value");
    let _ = page.update(&paths, Event::GlobalArch(Arch::Win32));
    let _ = page.update(&paths, Event::Revert);
    assert_eq!(global_editor(&page).new_env, EnvRow::default());
    assert!(!page.is_dirty());
}

#[test]
fn external_runners_switch_reads_and_writes_the_global_key() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    assert_eq!(global_editor(&page).form.draft.external_runners, None);
    let _ = page.update(&paths, Event::GlobalExternalRunners(true));
    assert_eq!(page.changes(), 1);
    let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
    assert!(report.failed.is_none());
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(true)
    );

    let mut page = global_page(&paths);
    assert_eq!(global_editor(&page).form.draft.external_runners, Some(true));
    let _ = page.update(&paths, Event::GlobalExternalRunners(false));
    load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(false)
    );
}

#[test]
fn an_alias_resolving_into_steams_folder_is_labeled_steam() {
    let (root, paths) = setup();
    let steam = crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let tools = steam.parent().unwrap();
    write_global(
        &paths,
        &format!("[defaults]\nrunner_root = '{}'\n", tools.display()),
    );
    let global = GlobalConfig::load(&paths).unwrap();
    let (choices, _) = load::runner_choices(&paths, &global, "GE-Proton-Latest");
    let alias = choices
        .iter()
        .find(|choice| choice.name == "GE-Proton-Latest")
        .unwrap();
    assert_eq!(
        alias.launcher,
        Some(wineo_core::external_runners::Source::Steam)
    );
    assert_eq!(alias.proton, Some(true));
    let missing = choices
        .iter()
        .find(|choice| choice.name == "Proton-CachyOS-Latest")
        .unwrap();
    assert_eq!(missing.launcher, None);
    assert_eq!(missing.proton, None);
}

#[test]
fn external_runners_switch_does_nothing_in_a_portable_folder() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = root.path().join("portable");
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&portable),
    )
    .unwrap();
    let mut page = global_page(&paths);
    let _ = page.update(&paths, Event::GlobalExternalRunners(true));
    assert!(!page.is_dirty());
    assert_eq!(global_editor(&page).form.draft.external_runners, None);
}

#[test]
fn portable_prefix_root_failure_is_shown_in_the_global_save_bar() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = root.path().join("portable");
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&portable),
    )
    .unwrap();
    let mut page = global_page(&paths);
    let _ = page.update(
        &paths,
        Event::GlobalField(global::Field::PrefixRoot, "/tmp/outside".into()),
    );
    assert!(page.can_save());
    let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
    assert!(report.failed.is_some());
    let _ = page.update(&paths, Event::Save);
    let _ = page.update(&paths, Event::Saved(1, Ok(report)));
    assert!(page.error.is_some());
    assert!(page.is_dirty());
}

#[test]
fn commented_global_round_trip_changes_one_control_per_section() {
    let (_root, paths) = setup();
    let before = "# global\nschema_version = 1\ngamemode = false # launch\nstop_timeout = 5 # seconds\n\n[defaults]\narch = \"win64\" # new\n\n[art]\nfetch = \"all\" # art\n\n[env]\nA = \"old\" # env\n";
    write_global(&paths, before);
    let mut page = global_page(&paths);
    for event in [
        Event::GlobalSwitch(Switch::Gamemode, true),
        Event::GlobalArch(Arch::Win32),
        Event::GlobalField(global::Field::StopTimeout, "6".into()),
        Event::GlobalArt(ArtFetch::Icon),
        Event::EnvValue(0, "new".into()),
    ] {
        let _ = page.update(&paths, event);
    }
    assert_eq!(page.changes(), 5);
    let report = load::save_global(&paths, &global_editor(&page).form.edits()).unwrap();
    assert_eq!(report.failed, None);
    let after = fs::read_to_string(paths.global_config()).unwrap();
    assert_eq!(
        after,
        before
            .replace("gamemode = false", "gamemode = true")
            .replace("stop_timeout = 5", "stop_timeout = 6")
            .replace("arch = \"win64\"", "arch = \"win32\"")
            .replace("fetch = \"all\"", "fetch = \"icon\"")
            .replace("A = \"old\"", "A = \"new\"")
    );
}

fn setup() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    (root, paths)
}

const GAME: &str = "# my game\nschema_version = 1\nprefix = \"~/prefixes/game\"\nrunner = \"missing\" # pinned\narch = \"win64\"\ngamemode = false # off here\n\n[gamescope]\nwidth = 1920 # native\n";

fn write(paths: &Paths, text: &str) {
    let path = paths.environment_config("game");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn write_global(paths: &Paths, text: &str) {
    let path = paths.global_config();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn land(page: &mut Page, paths: &Paths) {
    assert!(page.loading());
    let snapshot = load::snapshot(paths, "game");
    let _ = page.update(paths, Event::Loaded(1, snapshot));
}

fn page(paths: &Paths) -> Page {
    let mut page = Page::new("game".into(), 1);
    let _ = page.reload(paths, Mode::Replace);
    land(&mut page, paths);
    page
}

fn editor(page: &Page) -> &Editor {
    page.editor().expect("form")
}

/// Starts a check the way the tick does and hands back its sequence.
fn start_check(page: &mut Page) -> u64 {
    page.check_sequence += 1;
    page.checking = true;
    page.check_sequence
}

fn check(page: &mut Page, paths: &Paths) {
    let sequence = start_check(page);
    let _ = page.update(
        paths,
        Event::Checked(1, sequence, Texts::read(paths, "game")),
    );
}

/// Writes what the page would save, the way the worker does.
fn apply(page: &Page, paths: &Paths) -> wineo_core::settings::Applied {
    let edits = editor(page).form.edits();
    let pairs: Vec<(&str, &str)> = edits
        .pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let unset: Vec<&str> = edits.unset.iter().map(String::as_str).collect();
    settings::apply(paths, ConfigFile::Environment("game"), &pairs, &unset).unwrap()
}

fn saved(page: &mut Page, paths: &Paths, lines: &[&str], failed: Option<&str>) {
    page.saving = true;
    let report = Report {
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        failed: failed.map(str::to_owned),
        checkup: false,
    };
    let _ = page.update(paths, Event::Saved(1, Ok(report)));
}

#[test]
fn loads_file_values_and_tracks_changes() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let draft = &editor(&page).form.draft;
    assert_eq!(draft.gamemode, Some(false));
    assert!(draft.gamescope);
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let _ = page.update(&paths, Event::Reset(Switch::Gamemode));
    assert_eq!(page.changes(), 2);
    let edits = editor(&page).form.edits();
    assert_eq!(edits.pairs, [("mangohud".to_owned(), "true".to_owned())]);
    assert_eq!(edits.unset, ["gamemode"]);
    let _ = page.update(&paths, Event::Revert);
    assert!(!page.is_dirty());
}

#[test]
fn leaving_with_edits_asks_first() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    assert!(page.update(&paths, Event::Back).close);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    assert!(!page.update(&paths, Event::Back).close);
    assert!(page.leave_prompt);
    assert!(!page.update(&paths, Event::KeepEditing).close);
    assert!(!page.leave_prompt);
    assert!(page.is_dirty());
    let _ = page.update(&paths, Event::Back);
    assert!(page.update(&paths, Event::Discard).close);
}

#[test]
fn the_leave_prompt_goes_when_the_page_is_clean_or_saving() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let _ = page.update(&paths, Event::Back);
    let _ = page.update(&paths, Event::Revert);
    assert!(!page.leave_prompt);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let _ = page.update(&paths, Event::Back);
    let _ = page.update(&paths, Event::Save);
    assert!(page.saving && !page.leave_prompt);
    page.leave_prompt = true;
    assert!(!page.update(&paths, Event::Discard).close);
}

#[test]
fn a_file_change_reloads_quietly_without_edits() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    land(&mut page, &paths);
    assert!(!page.changed_on_disk);
    assert_eq!(editor(&page).form.draft.gamemode, Some(true));
    // The same text again is no change.
    check(&mut page, &paths);
    assert!(!page.loading());
}

#[test]
fn a_file_change_under_edits_asks_for_reload() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    assert!(page.changed_on_disk && !page.loading());
    assert_eq!(editor(&page).form.draft.mangohud, Some(true));
    // Ctrl+S never throws edits away.
    let _ = page.update(&paths, Event::Save);
    assert!(!page.loading() && !page.saving);
    let _ = page.update(&paths, Event::Reload);
    land(&mut page, &paths);
    assert!(!page.changed_on_disk);
    assert!(!page.is_dirty());
    assert_eq!(editor(&page).form.draft.gamemode, Some(true));
}

#[test]
fn edits_made_during_a_quiet_reload_are_kept() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    land(&mut page, &paths);
    assert!(page.changed_on_disk);
    assert_eq!(editor(&page).form.draft.mangohud, Some(true));
}

#[test]
fn a_stale_load_is_ignored() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.reload(&paths, Mode::Replace);
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    let snapshot = load::snapshot(&paths, "game");
    let _ = page.update(&paths, Event::Loaded(7, snapshot));
    assert!(page.loading());
    assert_eq!(editor(&page).form.draft.gamemode, Some(false));
}

#[test]
fn results_from_another_page_are_ignored() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    page.saving = true;
    let _ = page.update(&paths, Event::Saved(7, Err("old".into())));
    assert!(page.saving);
    assert_eq!(page.error, None);
}

#[test]
fn a_save_reloads_the_page_from_disk() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    apply(&page, &paths);
    saved(&mut page, &paths, &["done"], None);
    assert!(page.loading());
    land(&mut page, &paths);
    assert!(!page.is_dirty() && !page.changed_on_disk);
    assert_eq!(editor(&page).form.loaded.mangohud, Some(true));
    assert_eq!(page.notes, ["done"]);
    check(&mut page, &paths);
    assert!(!page.loading() && !page.changed_on_disk);
}

#[test]
fn a_save_restarts_an_open_add_form_scan() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::AddOpen(true));
    let old_sequence = page.scan_sequence;
    let _ = page.update(&paths, Event::Scanned(1, old_sequence, Scan::NoPrefix));
    assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    write(
        &paths,
        &TARGETED.replace("~/prefixes/game", "~/prefixes/other"),
    );
    saved(&mut page, &paths, &[], None);
    land(&mut page, &paths);
    assert!(editor(&page).add_open);
    assert_eq!(editor(&page).scan, Some(Scan::Scanning));
    let new_sequence = page.scan_sequence;
    assert_ne!(new_sequence, old_sequence);
    let _ = page.update(
        &paths,
        Event::Scanned(1, old_sequence, Scan::Failed("old prefix".into())),
    );
    assert_eq!(editor(&page).scan, Some(Scan::Scanning));
    let _ = page.update(
        &paths,
        Event::Scanned(1, new_sequence, load::scan(&paths, "game")),
    );
    assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
    let _ = page.update(&paths, Event::AddOpen(false));
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    saved(&mut page, &paths, &[], None);
    land(&mut page, &paths);
    assert!(!editor(&page).add_open);
    assert_eq!(editor(&page).scan, None);
}

#[test]
fn controls_are_locked_until_a_save_has_reloaded() {
    let (_root, paths) = setup();
    write(
        &paths,
        &format!(
            "{}\n[targets.game]\nexe = \"/bin/game\"\n[targets.game.gamescope]\nwidth = 5\n",
            GAME.replace(
                "[gamescope]",
                "winetricks = [\"corefonts\"]\n\n[env]\nA = \"1\"\n\n[gamescope]",
            )
        ),
    );
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let _ = page.update(&paths, Event::Save);
    assert!(page.saving && page.locked());
    let edits = [
        Event::Switch(Switch::Gamemode, true),
        Event::Reset(Switch::Mangohud),
        Event::Gamescope(false),
        Event::FieldText(Field::Width, "1".into()),
        Event::FieldChoice(Field::Filter, Choice(Some("fsr"))),
        Event::GrabCursor(true),
        Event::ArgsInput("-x".into()),
        Event::ArgsAdd,
        Event::RunnerPicked(RunnerChoice {
            name: "other".into(),
            launcher: None,
            proton: None,
        }),
        Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
        Event::BuildRemoved(Component::Dxvk),
        Event::ListInput(List::Tags, "rpg".into()),
        Event::ListAdd(List::Tags),
        Event::ListRemove(List::Winetricks, 0),
        Event::EnvKey(0, "B".into()),
        Event::EnvValue(0, "2".into()),
        Event::EnvRemove(0),
        Event::NewEnvKey("C".into()),
        Event::NewEnvValue("3".into()),
        Event::EnvAdd,
        Event::Prefix("~/elsewhere".into()),
        Event::GameId("umu-1".into()),
        Event::Store("gog".into()),
        Event::ProgramPicked(ProgramChoice {
            name: "Tool".into(),
            exe: "/bin/tool".into(),
        }),
        Event::NewTargetName("tool".into()),
        Event::NewTargetPath("/bin/tool".into()),
        Event::TargetAdd,
    ]
    .into_iter()
    .chain(
        [
            TargetEdit::Exe("/bin/other".into()),
            TargetEdit::Cwd("/tmp".into()),
            TargetEdit::Name("Game".into()),
            TargetEdit::Desktop(true),
            TargetEdit::Default(true),
            TargetEdit::Switch(Switch::Gamemode, true),
            TargetEdit::Reset(Switch::Gamemode),
            TargetEdit::FieldText(Field::Width, "6".into()),
            TargetEdit::FieldChoice(Field::Filter, Choice(Some("fsr"))),
            TargetEdit::GrabCursor(true),
            TargetEdit::GamescopeArgsInput("-f".into()),
            TargetEdit::GamescopeArgsAdd,
            TargetEdit::GamescopeArgsRemove(0),
            TargetEdit::ArgsInput("-x".into()),
            TargetEdit::ArgsAdd,
            TargetEdit::GamescopeHere,
            TargetEdit::Gamescope(false),
            TargetEdit::GamescopeReset,
            TargetEdit::AskRemove,
            TargetEdit::Remove,
        ]
        .map(|edit| Event::Target("game".into(), edit)),
    )
    .chain([Event::Revert])
    .collect::<Vec<_>>();
    for event in edits.clone() {
        let _ = page.update(&paths, event);
    }
    apply(&page, &paths);
    saved(&mut page, &paths, &[], None);
    assert!(page.loading() && page.locked());
    for event in edits {
        let _ = page.update(&paths, event);
    }
    land(&mut page, &paths);
    assert!(!page.locked());
    assert!(!page.is_dirty() && !page.changed_on_disk);
    let draft = &editor(&page).form.draft;
    assert_eq!((draft.mangohud, draft.gamemode), (Some(true), Some(false)));
    assert!(draft.gamescope && draft.fields.args.is_empty());
    assert_eq!(
        (&draft.dxvk, &draft.prefix),
        (&None, &"~/prefixes/game".to_owned())
    );
    assert!(draft.tags.is_empty() && draft.gameid.is_empty() && draft.store.is_empty());
    assert_eq!(draft.winetricks, ["corefonts"]);
    assert_eq!(
        draft.env,
        [EnvRow {
            key: "A".into(),
            value: "1".into()
        }]
    );
    let game = &draft.targets["game"];
    assert_eq!(game.exe, "/bin/game");
    assert_eq!(game.gamescope, form::TargetGamescope::On);
    assert_eq!(game.fields.width, "5");
    assert!(game.args.is_empty() && game.cwd.is_empty() && game.gamemode.is_none());
    assert_eq!(draft.targets.len(), 1);
    let editor = editor(&page);
    assert!(editor.args_input.is_empty() && editor.tags_input.is_empty());
    assert_eq!(editor.new_env, EnvRow::default());
    assert_eq!(editor.new_target, NewTarget::default());
    assert!(editor.target_args_input.is_empty());
    assert!(editor.target_gamescope_args_input.is_empty());
    assert_eq!(editor.removing, None);
}

#[test]
fn changing_the_file_back_clears_the_reload_state() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    assert!(page.changed_on_disk && !page.can_save());
    write(&paths, GAME);
    check(&mut page, &paths);
    assert!(!page.changed_on_disk && page.can_save() && !page.can_reload());
    assert!(page.is_dirty());
}

#[test]
fn a_check_that_started_before_a_load_is_dropped() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let sequence = start_check(&mut page);
    let stale = Texts::read(&paths, "game");
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    land(&mut page, &paths);
    assert_eq!(editor(&page).form.draft.gamemode, Some(true));
    // The old file text, read before the reload, would look like a change.
    let _ = page.update(&paths, Event::Checked(1, sequence, stale));
    assert!(!page.loading() && !page.changed_on_disk);
    // A save drops checks in flight too.
    let sequence = start_check(&mut page);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let _ = page.update(&paths, Event::Save);
    let _ = page.update(&paths, Event::CheckFailed(1, sequence));
    assert!(!page.checking);
    let _ = page.update(&paths, Event::Saved(1, Err("busy".into())));
    let _ = page.update(&paths, Event::Checked(1, sequence, Texts::default()));
    assert!(!page.loading() && !page.changed_on_disk);
}

#[test]
fn a_reload_closes_the_menu_state() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::MenuOpened);
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    assert!(!page.menu_open);
    let _ = page.update(&paths, Event::MenuOpened);
    land(&mut page, &paths);
    assert!(!page.menu_open);
}

#[test]
fn a_failed_save_keeps_edits_and_shows_the_error() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    page.saving = true;
    let _ = page.update(&paths, Event::Saved(1, Err("Environment is busy".into())));
    assert!(!page.saving && !page.loading());
    assert_eq!(page.error.as_deref(), Some("Environment is busy"));
    assert!(page.is_dirty());
    // Failed before the write: the edits are still edits.
    saved(&mut page, &paths, &["Installed DXVK 2.4"], Some("bad"));
    land(&mut page, &paths);
    assert_eq!(page.notes, ["Installed DXVK 2.4"]);
    assert_eq!(page.error.as_deref(), Some("bad"));
    assert!(page.is_dirty() && !page.changed_on_disk);
}

#[test]
fn a_save_that_failed_after_writing_is_not_an_outside_change() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    apply(&page, &paths);
    saved(&mut page, &paths, &[], Some("desktop entries failed"));
    land(&mut page, &paths);
    assert!(!page.is_dirty() && !page.changed_on_disk);
    assert_eq!(page.error.as_deref(), Some("desktop entries failed"));
    check(&mut page, &paths);
    assert!(!page.loading() && !page.changed_on_disk);
}

#[test]
fn crossing_runner_kinds_is_refused_before_save() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    if let Some(editor) = page.editor_mut() {
        editor.info.current = Some(Current {
            name: "wine-tkg".into(),
            proton: false,
        });
    }
    let proton = RunnerChoice {
        name: "GE-Proton9-20".into(),
        launcher: None,
        proton: Some(true),
    };
    let _ = page.update(&paths, Event::RunnerPicked(proton));
    let editor = editor(&page);
    assert_eq!(editor.form.draft.runner.as_deref(), Some("missing"));
    assert_eq!(
        editor.refusal.as_deref(),
        Some("Can't switch between Wine and Proton runners. Make a new environment instead.")
    );
    assert!(!page.is_dirty());
    let wine = RunnerChoice {
        name: "wine-ge".into(),
        launcher: None,
        proton: Some(false),
    };
    let _ = page.update(&paths, Event::RunnerPicked(wine));
    assert_eq!(page.editor().unwrap().refusal, None);
    assert_eq!(
        page.editor().unwrap().form.draft.runner.as_deref(),
        Some("wine-ge")
    );
    // An unresolvable runner is left for save to check.
    let unknown = RunnerChoice {
        name: "gone".into(),
        launcher: None,
        proton: None,
    };
    assert!(!refused(
        page.editor().unwrap().info.current.as_ref(),
        &unknown
    ));
}

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(path, "#!/bin/sh\n");
}

#[test]
fn a_kind_mismatch_at_save_gets_the_gui_wording() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    write(&paths, &GAME.replace("\"missing\"", "\"wine-tkg\""));
    let edits = form::Edits {
        pairs: vec![("runner".into(), "GE-Proton9-20".into())],
        unset: Vec::new(),
    };
    assert_eq!(
        load::save(&paths, "game", &edits),
        Err(load::KIND_MISMATCH.to_owned())
    );
}

#[test]
fn a_proxied_environment_gets_the_reduced_form() {
    let (_root, paths) = setup();
    write_global(&paths, "mangohud = [");
    write(
        &paths,
        "# Imported Steam game\nkind = 'proxied' # launcher-owned\nlauncher = 'steam'\napp_id = '1086940'\ntitle = 'Old title' # keep title note\ntags = ['rpg'] # keep tag note\ndesktop = false # keep switch note\n",
    );
    let mut page = page(&paths);
    assert!(page.global_warning().is_none());
    let original = fs::read_to_string(paths.environment_config("game")).unwrap();
    let no_op = form::Edits::default();
    load::save(&paths, "game", &no_op).unwrap();
    assert_eq!(
        fs::read_to_string(paths.environment_config("game")).unwrap(),
        original
    );

    let draft = &editor(&page).form.draft;
    assert_eq!(draft.tags, ["rpg"]);
    assert_eq!(draft.title.as_deref(), Some("Old title"));
    let proxied = draft.proxied.as_ref().expect("proxied settings");
    assert!(!proxied.desktop);

    for event in [
        Event::ListInput(List::Tags, "coop".to_owned()),
        Event::ListAdd(List::Tags),
        Event::ProxiedDesktop(true),
    ] {
        let _ = page.update(&paths, event);
    }
    assert_eq!(page.changes(), 2);
    assert!(page.can_save());
    let edits = editor(&page).form.edits();
    assert_eq!(
        edits.pairs,
        [
            ("tags".to_owned(), "[\"rpg\", \"coop\"]".to_owned()),
            ("desktop".to_owned(), "true".to_owned()),
        ]
    );
    assert!(edits.unset.is_empty());
    let report = load::save(&paths, "game", &edits).unwrap();
    assert_eq!(report.failed, None);

    let saved = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(saved.contains("# Imported Steam game"));
    assert!(saved.contains("kind = 'proxied' # launcher-owned"));
    assert!(saved.contains("# keep title note"));
    assert!(saved.contains("# keep tag note"));
    assert!(saved.contains("# keep switch note"));
    let config =
        wineo_core::config::EnvironmentConfig::parse(&paths.environment_config("game"), &saved)
            .unwrap();
    assert_eq!(config.tags, ["rpg", "coop"]);
    let proxied = config.proxied().unwrap();
    assert_eq!(proxied.title.as_deref(), Some("Old title"));
    assert!(proxied.desktop);
}

#[test]
fn a_steam_title_is_read_only_in_settings() {
    let (_root, paths) = setup();
    let text = "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\ntitle = 'Old title'\n";
    write(&paths, text);
    let mut page = page(&paths);
    for typed in ["", "New title"] {
        let _ = page.update(&paths, Event::Title(typed.to_owned()));
        assert_eq!(page.changes(), 0, "{typed}");
        assert!(!page.can_save());
        assert_eq!(editor(&page).form.draft.title.as_deref(), Some("Old title"));
        assert_eq!(editor(&page).form.edits(), form::Edits::default());
    }
    assert_eq!(
        fs::read_to_string(paths.environment_config("game")).unwrap(),
        text
    );
}

const NATIVE_GAME: &str = "# my game\nkind = 'native' # mine\n";

fn saved_title(paths: &Paths) -> (String, Option<String>) {
    let path = paths.environment_config("game");
    let text = fs::read_to_string(&path).unwrap();
    let title = wineo_core::config::EnvironmentConfig::parse(&path, &text)
        .unwrap()
        .title;
    (text, title)
}

#[test]
fn a_title_is_tracked_saved_and_cleared_on_wine_and_native() {
    for original in [GAME, NATIVE_GAME] {
        let (_root, paths) = setup();
        write(&paths, original);
        let mut new = page(&paths);
        assert_eq!(editor(&new).form.draft.title, None);

        let _ = new.update(&paths, Event::Title("  Dusk: Ascent \u{1f3ae} ".to_owned()));
        assert_eq!(new.changes(), 1);
        assert!(new.can_save());
        let edits = editor(&new).form.edits();
        assert_eq!(
            edits.pairs,
            [("title".to_owned(), "Dusk: Ascent \u{1f3ae}".to_owned())]
        );
        assert!(edits.unset.is_empty());
        assert_eq!(load::save(&paths, "game", &edits).unwrap().failed, None);
        let (text, title) = saved_title(&paths);
        assert_eq!(title.as_deref(), Some("Dusk: Ascent \u{1f3ae}"));
        assert!(text.starts_with("# my game\n"), "{text}");

        let mut page = page(&paths);
        assert_eq!(
            editor(&page).form.draft.title.as_deref(),
            Some("Dusk: Ascent \u{1f3ae}")
        );
        // Only spaces trimming drops is no change.
        let _ = page.update(&paths, Event::Title(" Dusk: Ascent \u{1f3ae}".to_owned()));
        assert_eq!(page.changes(), 0);
        let _ = page.update(&paths, Event::Title("   ".to_owned()));
        assert_eq!(page.changes(), 1);
        let _ = page.update(&paths, Event::Title(String::new()));
        assert_eq!(page.changes(), 1);
        let edits = editor(&page).form.edits();
        assert!(edits.pairs.is_empty());
        assert_eq!(edits.unset, ["title"]);
        assert_eq!(load::save(&paths, "game", &edits).unwrap().failed, None);
        let (text, title) = saved_title(&paths);
        assert_eq!(title, None);
        assert!(!text.contains("title"), "{text}");
        assert!(text.starts_with("# my game\n"), "{text}");
    }
}

#[test]
fn a_title_that_breaks_the_rules_turns_save_off() {
    let (_root, paths) = setup();
    write(&paths, NATIVE_GAME);
    let mut page = page(&paths);
    let fits = "\u{e9}".repeat(120);
    let _ = page.update(&paths, Event::Title(fits.clone()));
    assert!(page.can_save());
    assert_eq!(page.problem(), None);
    for (typed, problem) in [
        (
            format!("{fits}\u{e9}"),
            "Invalid title: can't be longer than 120 characters.",
        ),
        (
            "Dusk\tAscent".to_owned(),
            "Invalid title: can't contain control characters.",
        ),
    ] {
        let _ = page.update(&paths, Event::Title(typed));
        assert_eq!(page.changes(), 1);
        assert!(!page.can_save());
        assert_eq!(page.problem().as_deref(), Some(problem));
    }
}

#[test]
fn a_hand_written_title_is_left_alone_until_edited() {
    let (_root, paths) = setup();
    let text = "# legacy\nkind = 'native'\ntitle = '  Padded  ' # mine\n";
    write(&paths, text);
    let mut page = page(&paths);
    assert_eq!(page.changes(), 0);
    assert_eq!(page.problem(), None);
    load::save(&paths, "game", &editor(&page).form.edits()).unwrap();
    assert_eq!(
        fs::read_to_string(paths.environment_config("game")).unwrap(),
        text
    );
    // An unrelated edit doesn't touch it either.
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    let edits = editor(&page).form.edits();
    assert_eq!(edits.pairs, [("mangohud".to_owned(), "true".to_owned())]);
    // Typing it trimmed is a change, and saves the trimmed title.
    let _ = page.update(&paths, Event::Title("Padded".to_owned()));
    assert_eq!(page.changes(), 2);
    let edits = editor(&page).form.edits();
    assert_eq!(edits.pairs[1], ("title".to_owned(), "Padded".to_owned()));
    load::save(&paths, "game", &edits).unwrap();
    let (saved, title) = saved_title(&paths);
    assert_eq!(title.as_deref(), Some("Padded"));
    assert!(saved.contains("title = \"Padded\" # mine"), "{saved}");
}

#[test]
fn a_typed_title_survives_a_change_on_disk() {
    let (_root, paths) = setup();
    write(&paths, NATIVE_GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Title("Mine".to_owned()));
    write(&paths, &format!("{NATIVE_GAME}title = 'Theirs'\n"));
    check(&mut page, &paths);
    assert!(page.changed_on_disk && !page.can_save());
    assert_eq!(editor(&page).form.draft.title.as_deref(), Some("Mine"));
    let _ = page.update(&paths, Event::Reload);
    land(&mut page, &paths);
    assert!(!page.changed_on_disk && !page.is_dirty());
    assert_eq!(editor(&page).form.draft.title.as_deref(), Some("Theirs"));

    // Without edits, a title changed on disk just reloads.
    write(&paths, &format!("{NATIVE_GAME}title = 'Third'\n"));
    check(&mut page, &paths);
    land(&mut page, &paths);
    assert!(!page.changed_on_disk);
    assert_eq!(editor(&page).form.draft.title.as_deref(), Some("Third"));
}

#[test]
fn an_unreadable_config_shows_its_error() {
    let (_root, paths) = setup();
    write(&paths, "this is [");
    let page = page(&paths);
    assert!(matches!(page.view, View::Error(_)));
    assert!(page.has_file());
    assert!(!page.can_save());
}

#[test]
fn a_broken_global_config_warns_and_reloads_once_fixed() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    write_global(&paths, "mangohud = [");
    let mut page = page(&paths);
    let warning = page.global_warning().expect("warning").to_owned();
    assert!(
        warning.contains(&paths.global_config().display().to_string()),
        "{warning}"
    );
    assert_eq!(editor(&page).info.inherited, form::Inherited::default());
    let _ = page.update(&paths, Event::Switch(Switch::Wayland, true));
    write_global(&paths, "mangohud = true\n");
    check(&mut page, &paths);
    land(&mut page, &paths);
    assert_eq!(page.global_warning(), None);
    assert_eq!(editor(&page).info.inherited.mangohud, Some(true));
    // A global change doesn't touch the environment's edits.
    assert_eq!(editor(&page).form.draft.wayland, Some(true));
    assert!(!page.changed_on_disk);
    check(&mut page, &paths);
    assert!(!page.loading());
}

#[test]
fn saving_one_control_changes_only_its_line() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Gamemode, true));
    let _ = page.update(&paths, Event::FieldText(Field::Width, "2560".into()));
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert_eq!(
        text,
        GAME.replace("gamemode = false", "gamemode = true")
            .replace("width = 1920", "width = 2560")
    );
    let before: Vec<&str> = GAME.lines().collect();
    let after: Vec<&str> = text.lines().collect();
    let changed = before
        .iter()
        .zip(&after)
        .filter(|(old, new)| old != new)
        .count();
    assert_eq!((before.len(), changed), (after.len(), 2));
}

#[test]
fn switching_gamescope_on_saves_an_empty_table() {
    let (_root, paths) = setup();
    let text = "schema_version = 1\nprefix = \"~/p\"\nrunner = \"missing\"\narch = \"win64\"\ngamescope = false\n";
    write(&paths, text);
    let mut edited = page(&paths);
    assert!(!editor(&edited).form.draft.gamescope);
    let _ = edited.update(&paths, Event::Gamescope(true));
    let applied = apply(&edited, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let written = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(written.ends_with("[gamescope]\n"), "{written}");
    let reloaded = page(&paths);
    assert!(editor(&reloaded).form.draft.gamescope);
    assert_eq!(
        editor(&reloaded).form.draft.fields,
        form::GamescopeFields::default()
    );
}

#[test]
fn switching_gamescope_off_removes_it_and_on_again_restores_fields() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Gamescope(false));
    apply(&page, &paths);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(!text.contains("gamescope"), "{text}");
    saved(&mut page, &paths, &[], None);
    land(&mut page, &paths);
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::Gamescope(true));
    assert_eq!(editor(&page).form.draft.fields.width, "1920");
    assert_eq!(page.changes(), 2);
}

#[test]
fn args_are_added_and_removed_as_chips() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::ArgsInput(String::new()));
    let _ = page.update(&paths, Event::ArgsAdd);
    assert!(!page.is_dirty());
    for arg in ["--backend", "sdl", "two words"] {
        let _ = page.update(&paths, Event::ArgsInput(arg.into()));
        let _ = page.update(&paths, Event::ArgsAdd);
    }
    let _ = page.update(&paths, Event::ArgsRemove(1));
    let _ = page.update(&paths, Event::ArgsRemove(9));
    assert_eq!(
        editor(&page).form.draft.fields.args,
        ["--backend", "two words"]
    );
    assert!(editor(&page).args_input.is_empty());
    apply(&page, &paths);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(
        text.contains("args = [\"--backend\", \"two words\"]"),
        "{text}"
    );
}

const FULL: &str = "# my game\nschema_version = 1\nprefix = \"~/prefixes/game\" # where it lives\nrunner = \"missing\" # pinned\narch = \"win64\"\ndxvk = \"2.3\" # pinned build\ntags = [\"rpg\"] # shelf\nwinetricks = [\"corefonts\"] # fonts\n\n[env]\n# chatty otherwise\nWINEDEBUG = \"-all\" # quiet\nDXVK_HUD = \"fps\"\n";

#[test]
fn saving_one_control_per_section_changes_only_those_lines() {
    let (root, paths) = setup();
    write(&paths, FULL);
    let mut page = page(&paths);
    let events = [
        Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
        Event::ListInput(List::Winetricks, "vcrun2019".into()),
        Event::ListAdd(List::Winetricks),
        Event::ListRemove(List::Tags, 0),
        Event::ListInput(List::Tags, "co-op".into()),
        Event::ListAdd(List::Tags),
        // Rows are in name order, so WINEDEBUG is second.
        Event::EnvValue(1, r#"a "b" \c = d"#.into()),
        Event::Prefix(root.path().join("Games/game").display().to_string()),
    ];
    for event in events {
        let _ = page.update(&paths, event);
    }
    assert_eq!(page.changes(), 5);
    assert!(page.can_save());
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    let before: Vec<&str> = FULL.lines().collect();
    let after: Vec<&str> = text.lines().collect();
    assert_eq!(before.len(), after.len(), "{text}");
    let changed: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(old, new)| old != new)
        .map(|(_, new)| *new)
        .collect();
    assert_eq!(
        changed[..4],
        [
            "prefix = \"~/Games/game\" # where it lives",
            "dxvk = \"latest\" # pinned build",
            "tags = [\"co-op\"] # shelf",
            "winetricks = [\"corefonts\", \"vcrun2019\"] # fonts",
        ]
    );
    assert!(changed[4].starts_with("WINEDEBUG = ") && changed[4].ends_with(" # quiet"));
    assert_eq!(changed.len(), 5);
    let config = wineo_core::environment::Environment::load(&paths, "game").unwrap();
    assert_eq!(config.config.env["WINEDEBUG"], r#"a "b" \c = d"#);
}

fn env(paths: &Paths) -> std::collections::BTreeMap<String, String> {
    wineo_core::environment::Environment::load(paths, "game")
        .unwrap()
        .config
        .env
}

#[test]
fn an_env_name_with_a_dot_and_a_quote_round_trips() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let key = r#"MY.VAR"X"#;
    let _ = page.update(&paths, Event::NewEnvKey(key.into()));
    let _ = page.update(&paths, Event::NewEnvValue("1".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    assert_eq!(
        editor(&page).form.edits().pairs,
        [(format!("env.{key}"), "1".to_owned())]
    );
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    assert_eq!(env(&paths)[key], "1");
    saved(&mut page, &paths, &[], None);
    land(&mut page, &paths);
    assert!(!page.is_dirty());
    assert_eq!(editor(&page).form.draft.env[0].key, key);
}

#[test]
fn swapping_two_env_names_sets_both_and_unsets_neither() {
    let (_root, paths) = setup();
    write(&paths, &format!("{GAME}\n[env]\nA = \"1\"\nB = \"2\"\n"));
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::EnvKey(0, "B".into()));
    // A duplicate for a moment, until the other row is renamed too.
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::EnvKey(1, "A".into()));
    assert!(page.can_save());
    let edits = editor(&page).form.edits();
    assert!(edits.unset.is_empty());
    assert_eq!(
        edits.pairs,
        [
            ("env.A".to_owned(), "2".to_owned()),
            ("env.B".to_owned(), "1".to_owned())
        ]
    );
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let written = env(&paths);
    assert_eq!((written["A"].as_str(), written["B"].as_str()), ("2", "1"));
    assert_eq!(written.len(), 2);
}

#[test]
fn a_native_page_takes_no_wine_only_edits() {
    let (_root, paths) = setup();
    write(&paths, "schema_version = 1\nkind = \"native\"\n");
    let mut page = page(&paths);
    for event in [
        Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
        Event::ListInput(List::Winetricks, "corefonts".into()),
        Event::ListAdd(List::Winetricks),
        Event::Prefix("~/p".into()),
        Event::GameId("umu-1".into()),
        Event::Store("gog".into()),
        Event::Switch(Switch::Wayland, true),
    ] {
        let _ = page.update(&paths, event);
    }
    assert!(!page.is_dirty());
    assert!(editor(&page).winetricks_input.is_empty());
    // Native programs may have their own WINEPREFIX.
    let _ = page.update(&paths, Event::NewEnvKey("WINEPREFIX".into()));
    let _ = page.update(&paths, Event::NewEnvValue("/x".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    let _ = page.update(&paths, Event::ListInput(List::Tags, "linux".into()));
    let _ = page.update(&paths, Event::ListAdd(List::Tags));
    assert_eq!(page.changes(), 2);
    apply(&page, &paths);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(text.contains("WINEPREFIX = \"/x\""), "{text}");
}

/// A wine build and a Proton build in the runner root, as `wineo runners`
/// would find them.
fn fake_runners(paths: &Paths) {
    let root = GlobalConfig::default().runner_root(paths).unwrap();
    for program in ["wine", "wineserver"] {
        executable(&root.join("wine-tkg/bin").join(program));
        executable(&root.join("GE-Proton9-20/files/bin").join(program));
    }
    executable(&root.join("GE-Proton9-20/proton"));
    fs::write(root.join("GE-Proton9-20/version"), "1 GE-Proton9-20\n").unwrap();
}

#[test]
fn proton_disables_components_and_shows_protonfixes() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    write(&paths, &GAME.replace("\"missing\"", "\"GE-Proton9-20\""));
    let mut page = page(&paths);
    assert!(editor(&page).proton());
    let _ = page.update(
        &paths,
        Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
    );
    assert!(!page.is_dirty());
    let shown = editor(&page);
    assert!(!shown.build_enabled("game", Component::Vkd3d));
    assert_eq!(
        shown.build_refusal("game", Component::Dxvk).as_deref(),
        Some(
            "`dxvk` only means something to wine, and `game` runs on GE-Proton9-20, which bundles its own"
        )
    );
    let _ = page.update(&paths, Event::GameId("umu-1".into()));
    assert_eq!(
        editor(&page).form.edits().pairs,
        [("gameid".to_owned(), "umu-1".to_owned())]
    );
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(text.contains("gameid = \"umu-1\""), "{text}");
}

#[test]
fn stale_builds_on_proton_can_only_be_removed() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    write(
        &paths,
        &GAME
            .replace("\"missing\"", "\"GE-Proton9-20\"")
            .replace("arch = ", "dxvk = \"latest\"\nvkd3d = \"latest\"\narch = "),
    );
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    // Core would refuse the save, so it doesn't get that far.
    let reason = page.problem().expect("problem");
    assert!(
        reason.starts_with("`dxvk` only means something to wine"),
        "{reason}"
    );
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::BuildPicked(Component::Vkd3d, Build(None)));
    assert_eq!(editor(&page).form.draft.vkd3d.as_deref(), Some("latest"));
    let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
    let shown = editor(&page);
    assert_eq!(
        shown.build_refusal.as_deref(),
        Some(form::REMOVE_VKD3D_FIRST)
    );
    assert_eq!(shown.form.draft.dxvk.as_deref(), Some("latest"));
    let _ = page.update(&paths, Event::BuildRemoved(Component::Vkd3d));
    let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
    assert_eq!(editor(&page).build_refusal, None);
    assert_eq!(page.problem(), None);
    let edits = editor(&page).form.edits();
    assert_eq!(edits.unset, ["dxvk", "vkd3d"]);
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(!text.contains("dxvk") && !text.contains("vkd3d"), "{text}");
    // Nothing left to remove.
    let _ = page.update(&paths, Event::BuildRemoved(Component::Dxvk));
    assert_eq!(page.changes(), 3);
}

#[test]
fn proton_with_only_vkd3d_says_to_remove_it() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    write(
        &paths,
        &GAME
            .replace("\"missing\"", "\"GE-Proton9-20\"")
            .replace("arch = ", "vkd3d = \"latest\"\narch = "),
    );
    let mut page = page(&paths);
    let reason = page.problem().expect("problem");
    assert!(
        reason.starts_with("`vkd3d` only means something to wine"),
        "{reason}"
    );
    let _ = page.update(&paths, Event::BuildRemoved(Component::Vkd3d));
    assert_eq!(page.problem(), None);
    assert!(page.can_save());
}

#[test]
fn gating_follows_the_runner_picked_in_the_draft() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    // An unresolvable runner can go either way, so nothing is gated yet.
    write(&paths, GAME);
    let mut page = page(&paths);
    assert!(!editor(&page).proton());
    assert!(!editor(&page).protonfixes_shown());
    let proton = RunnerChoice {
        name: "GE-Proton9-20".into(),
        launcher: None,
        proton: Some(true),
    };
    let _ = page.update(&paths, Event::RunnerPicked(proton));
    let shown = editor(&page);
    assert!(shown.proton() && shown.protonfixes_shown());
    assert!(!shown.build_enabled("game", Component::Dxvk));
    let _ = page.update(&paths, Event::GameId("umu-1".into()));
    assert_eq!(editor(&page).form.draft.gameid, "umu-1");
    assert!(page.can_save());
    let wine = RunnerChoice {
        name: "wine-tkg".into(),
        launcher: None,
        proton: Some(false),
    };
    let _ = page.update(&paths, Event::RunnerPicked(wine));
    let shown = editor(&page);
    assert!(!shown.proton() && shown.protonfixes_shown());
    assert!(shown.build_enabled("game", Component::Dxvk));
    assert_eq!(
        page.problem().as_deref(),
        Some("Game id and store only apply to Proton runners. Pick a Proton runner or clear them.")
    );
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::GameId(String::new()));
    assert_eq!(page.problem(), None);
    assert!(!editor(&page).protonfixes_shown());
    let _ = page.update(
        &paths,
        Event::RunnerPicked(RunnerChoice {
            name: "GE-Proton9-20".into(),
            launcher: None,
            proton: Some(true),
        }),
    );
    let _ = page.update(&paths, Event::Store("gog".into()));
    let _ = page.update(
        &paths,
        Event::RunnerPicked(RunnerChoice {
            name: "wine-tkg".into(),
            launcher: None,
            proton: Some(false),
        }),
    );
    assert!(editor(&page).protonfixes_shown());
    assert!(!page.can_save());
    let _ = page.update(&paths, Event::Store(String::new()));
    assert_eq!(page.problem(), None);
}

#[test]
fn vkd3d_without_dxvk_blocks_saving_only_when_core_checks_it() {
    let (_root, paths) = setup();
    fake_runners(&paths);
    write(
        &paths,
        &GAME.replace("arch = ", "vkd3d = \"latest\"\narch = "),
    );
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    assert_eq!(page.problem(), None);
    assert!(page.can_save());
    let wine = RunnerChoice {
        name: "wine-tkg".into(),
        launcher: None,
        proton: Some(false),
    };
    let _ = page.update(&paths, Event::RunnerPicked(wine));
    assert_eq!(page.problem().as_deref(), Some(form::VKD3D_NEEDS_DXVK));
    assert!(!page.can_save());
}

#[test]
fn dropping_dxvk_under_vkd3d_is_refused_with_the_reason() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let vkd3d = Event::BuildPicked(Component::Vkd3d, Build(Some("latest".into())));
    let _ = page.update(&paths, vkd3d.clone());
    assert!(!page.is_dirty());
    let _ = page.update(
        &paths,
        Event::BuildPicked(Component::Dxvk, Build(Some("latest".into()))),
    );
    let _ = page.update(&paths, vkd3d);
    let _ = page.update(&paths, Event::BuildPicked(Component::Dxvk, Build(None)));
    let editor = editor(&page);
    assert_eq!(
        editor.build_refusal.as_deref(),
        Some(form::VKD3D_NEEDS_DXVK)
    );
    assert_eq!(editor.form.draft.dxvk.as_deref(), Some("latest"));
    assert!(page.can_save());
}

#[test]
fn text_typed_but_not_added_survives_a_quiet_reload() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let typing = [
        Event::ArgsInput("-e".into()),
        Event::ListInput(List::Winetricks, "corefonts".into()),
        Event::ListInput(List::Tags, "rpg".into()),
        Event::NewEnvKey("A".into()),
        Event::NewEnvValue("1".into()),
    ];
    for event in typing {
        let _ = page.update(&paths, event);
    }
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    land(&mut page, &paths);
    assert!(!page.changed_on_disk);
    let shown = editor(&page);
    assert_eq!(shown.form.draft.gamemode, Some(true));
    assert_eq!(
        (
            shown.args_input.as_str(),
            shown.winetricks_input.as_str(),
            shown.tags_input.as_str()
        ),
        ("-e", "corefonts", "rpg")
    );
    assert_eq!(
        (shown.new_env.key.as_str(), shown.new_env.value.as_str()),
        ("A", "1")
    );
    // Typing while the reload runs lands too.
    write(&paths, GAME);
    check(&mut page, &paths);
    let _ = page.update(&paths, Event::ListInput(List::Tags, "co-op".into()));
    land(&mut page, &paths);
    assert_eq!(editor(&page).tags_input, "co-op");
    // Reload after an outside change throws it away with the edits.
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, true));
    write(&paths, &GAME.replace("gamemode = false", "gamemode = true"));
    check(&mut page, &paths);
    let _ = page.update(&paths, Event::Reload);
    land(&mut page, &paths);
    assert!(editor(&page).tags_input.is_empty());
}

#[test]
fn a_relative_prefix_turns_save_off() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::Prefix("prefixes/game2".into()));
    assert!(page.is_dirty() && !page.can_save());
    assert_eq!(
        page.problem().as_deref(),
        Some("The prefix must be absolute or start with ~/.")
    );
    let _ = page.update(&paths, Event::Prefix("~/prefixes/game2".into()));
    assert!(page.can_save());
}

#[test]
fn revert_clears_text_typed_but_not_added() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    for event in [
        Event::ArgsInput("-e".into()),
        Event::ListInput(List::Winetricks, "corefonts".into()),
        Event::ListInput(List::Tags, "rpg".into()),
        Event::NewEnvKey("A".into()),
        Event::NewEnvValue("1".into()),
        Event::Switch(Switch::Mangohud, true),
        Event::Revert,
    ] {
        let _ = page.update(&paths, event);
    }
    let editor = editor(&page);
    assert!(editor.args_input.is_empty() && editor.winetricks_input.is_empty());
    assert!(editor.tags_input.is_empty());
    assert_eq!(editor.new_env, EnvRow::default());
}

#[test]
fn chips_and_env_rows_refuse_bad_input() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut page = page(&paths);
    for (list, bad) in [(List::Winetricks, "-q"), (List::Tags, "a b")] {
        let _ = page.update(&paths, Event::ListInput(list, bad.into()));
        let _ = page.update(&paths, Event::ListAdd(list));
    }
    let _ = page.update(&paths, Event::NewEnvKey("WINEPREFIX".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    assert!(!page.is_dirty());
    let _ = page.update(&paths, Event::NewEnvKey("A".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    let _ = page.update(&paths, Event::NewEnvKey("A".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    assert_eq!(editor(&page).form.draft.env.len(), 1);
    assert_eq!(editor(&page).new_env.key, "A");
    // Renaming a row onto another's name blocks Save until it's fixed.
    let _ = page.update(&paths, Event::NewEnvKey("B".into()));
    let _ = page.update(&paths, Event::EnvAdd);
    let _ = page.update(&paths, Event::EnvKey(1, "A".into()));
    assert!(page.is_dirty() && !page.can_save());
    let _ = page.update(&paths, Event::Save);
    assert!(!page.saving);
    let _ = page.update(&paths, Event::EnvKey(1, "B".into()));
    assert!(page.can_save());
}

fn open_app(paths: &Paths) -> App {
    let mut app = App::boot(paths.clone()).0;
    app.selected = Some("game".into());
    let _ = app.update(Message::OpenSettings);
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot(paths, "game"),
    )));
    app
}

fn open_global_app(paths: &Paths) -> App {
    let mut app = App::boot(paths.clone()).0;
    let _ = app.open_global_settings();
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(paths),
    )));
    app
}

fn steam_saved(ok: bool) -> Result<steam_key::Saved, String> {
    if ok {
        Ok(steam_key::Saved {
            status: steam_key::Status {
                source: Some(wineo_core::key_store::KeySource::Keyring),
                override_note: false,
            },
            warning: None,
            result: Some("Saved the Steam Web API key to the keyring".to_owned()),
        })
    } else {
        Err("Couldn't check the key: Steam rejected the Web API key".to_owned())
    }
}

/// Starts a Steam key save on the open global page and returns its
/// generation, sequence, and input session.
fn start_steam_save(app: &mut App) -> (u64, u64, Option<u64>) {
    let _ = app.update(Message::Settings(Event::SteamKeySetOpen));
    let _ = app.update(Message::Settings(Event::SteamKeyText(
        steamgriddb::Secret::new(steam_key::tests::KEY.to_owned()),
    )));
    let _ = app.update(Message::Settings(Event::SteamKeySet));
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Saving));
    assert!(app.steam_key_work.pending());
    (
        page.generation,
        page.steam_key_busy_sequence.unwrap(),
        page.steam_key_input_session,
    )
}

fn reopen_global(app: &mut App, paths: &Paths) {
    let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(paths),
    )));
}

fn assert_steam_outcome(app: &App, paths: &Paths, ok: bool) {
    let page = app.settings.as_ref().unwrap();
    assert!(page.steam_key.busy.is_none());
    assert!(!app.steam_key_work.pending());
    let shown = view::steam_key_row(&page.steam_key, paths).texts();
    if ok {
        assert_eq!(shown[1], "Saved in the keyring");
        assert!(
            shown
                .iter()
                .any(|text| text == "Saved the Steam Web API key to the keyring")
        );
    } else {
        assert!(
            shown
                .iter()
                .any(|text| text == "Couldn't check the key: Steam rejected the Web API key")
        );
    }
    assert!(
        !shown
            .iter()
            .any(|text| text.contains(steam_key::tests::KEY))
    );
}

#[test]
fn steam_key_result_lands_after_leaving_and_returning_to_settings() {
    use crate::sidebar::Section;
    for ok in [true, false] {
        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let (generation, sequence, session) = start_steam_save(&mut app);
        let _ = app.update(Message::Section(Section::Trash));
        assert!(app.settings.is_none());
        let _ = app.update(Message::Settings(Event::SteamKeySaved(
            generation,
            sequence,
            session,
            steam_saved(ok),
        )));
        assert!(!app.steam_key_work.pending());
        reopen_global(&mut app, &paths);
        assert_steam_outcome(&app, &paths, ok);
    }
}

/// A GOG logout outlives the page that started it: its result shows
/// when the page opens again, and nothing else starts meanwhile.
#[test]
fn gog_logout_result_lands_after_leaving_and_returning() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Gog(gog::Event::Logout)));
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.gog.busy, Some(gog::Busy::LoggingOut));
    assert!(app.gog_work.pending());
    let (generation, sequence) = (page.generation, page.gog.busy_sequence.unwrap());
    let _ = app.update(Message::Section(Section::Trash));
    assert!(app.settings.is_none());
    reopen_global(&mut app, &paths);
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.gog.busy, Some(gog::Busy::LoggingOut));
    let _ = app.update(Message::Settings(Event::Gog(gog::Event::LoginOpen)));
    assert_eq!(
        app.settings.as_ref().unwrap().gog.busy,
        Some(gog::Busy::LoggingOut)
    );
    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Settings(Event::Gog(gog::Event::LoggedOut(
        generation,
        sequence,
        None,
        Ok(gog::LoggedOut {
            status: gog::Status::LoggedOut,
            result: "Removed the GOG login.".to_owned(),
        }),
    ))));
    assert!(!app.gog_work.pending());
    reopen_global(&mut app, &paths);
    let page = app.settings.as_ref().unwrap();
    assert_eq!(page.gog.busy, None);
    let texts = view::gog_texts(&page.gog);
    assert!(texts.contains(&"Removed the GOG login.".to_owned()));
    assert!(texts.contains(&"Not logged in".to_owned()));
}

/// Like a Steam or SteamGridDB key being saved, a GOG login or logout,
/// and the startup update check, never hold the window open.
#[test]
fn gog_account_work_and_the_startup_check_never_hold_the_window_open() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Gog(gog::Event::Logout)));
    assert!(app.gog_work.pending());
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(!app.close_prompt);

    let mut app = App::boot(paths.clone()).0;
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(!app.close_prompt);
}

/// Logging in works from a portable session too: GOG games live in the
/// host library, but the login is the same.
#[test]
fn the_gog_section_shows_in_a_portable_session() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| std::ffi::OsString::from(&home)),
        Some(&root.path().join("portable")),
    )
    .unwrap();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::Gog(gog::Event::LoginOpen)));
    assert_eq!(
        app.settings.as_ref().unwrap().gog.busy,
        Some(gog::Busy::Opening)
    );
    let mut view = app
        .settings
        .as_ref()
        .unwrap()
        .view(&paths, crate::theme::Palette::DARK);
    let _ = crate::layout_probe::layout(&mut view, iced::Size::new(900.0, 700.0));
    assert!(view::gog_texts(&app.settings.as_ref().unwrap().gog).contains(&"Log in".to_owned()));
}

#[test]
fn a_reopened_page_waits_for_the_pending_steam_key_save() {
    use crate::sidebar::Section;
    for ok in [true, false] {
        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let (generation, sequence, session) = start_steam_save(&mut app);
        let _ = app.update(Message::Section(Section::Trash));
        reopen_global(&mut app, &paths);
        let page = app.settings.as_ref().unwrap();
        assert_ne!(page.generation, generation);
        assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Saving));
        assert!(app.spinning());

        // No second write starts while the first is pending.
        let _ = app.update(Message::Settings(Event::SteamKeyRemove));
        let _ = app.update(Message::Settings(Event::SteamKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamKeySet));
        let page = app.settings.as_ref().unwrap();
        assert_eq!(page.steam_key.busy, Some(steamgriddb::Busy::Saving));
        assert_eq!(page.steam_key_busy_sequence, Some(sequence));

        let _ = app.update(Message::Settings(Event::SteamKeySaved(
            generation,
            sequence,
            session,
            steam_saved(ok),
        )));
        assert_steam_outcome(&app, &paths, ok);
        assert!(!app.spinning());
    }
}

#[test]
fn a_pending_steam_key_save_refuses_another_write_even_from_a_fresh_page() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let (_, sequence, _) = start_steam_save(&mut app);
    // A page that somehow didn't adopt the work still can't start one.
    app.settings.as_mut().unwrap().steam_key.busy = None;
    let _ = app.update(Message::Settings(Event::SteamKeyRemove));
    let page = app.settings.as_ref().unwrap();
    assert!(page.steam_key.busy.is_none());
    assert!(app.steam_key_work.pending());
    assert_eq!(page.steam_key_busy_sequence, Some(sequence));
}

#[test]
fn key_work_is_busy_only_while_it_runs() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    assert!(!app.spinning());
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyRemove));
    assert!(app.settings.as_ref().unwrap().key_busy());
    assert!(app.spinning());
    let page = app.settings.as_mut().unwrap();
    page.steamgriddb.busy = None;
    assert!(!app.spinning());
    let _ = app.update(Message::Settings(Event::SteamKeyRemove));
    assert!(app.spinning());
}

#[test]
fn key_save_result_lands_after_leaving_and_returning_to_global_settings() {
    use crate::sidebar::Section;

    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
        steamgriddb::Secret::new("private-value".to_owned()),
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
    let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

    let _ = app.update(Message::Section(Section::Trash));
    assert!(app.settings.is_none());
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
        generation,
        sequence,
        input_session,
        Ok(steamgriddb::Saved {
            status: steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            },
            warning: None,
            result: Some("Saved the SteamGridDB key to the keyring".to_owned()),
        }),
    )));
    assert!(app.steamgriddb_late_result.is_some());

    let _ = app.update(Message::Section(Section::Settings));

    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
        generation,
        sequence,
        Ok(steamgriddb::Status {
            source: Some(wineo_core::steamgriddb::KeySource::Keyring),
            override_note: false,
        }),
    )));

    let page = app.settings.as_ref().unwrap();
    assert!(page.is_global());
    assert_eq!(
        page.steamgriddb.result.as_deref(),
        Some("Saved the SteamGridDB key to the keyring")
    );
    assert_eq!(
        page.steamgriddb.status.as_ref().unwrap().source,
        Some(wineo_core::steamgriddb::KeySource::Keyring)
    );
    assert!(app.steamgriddb_late_result.is_none());
}

#[test]
fn failed_key_save_result_returns_with_an_empty_input_and_error() {
    use crate::sidebar::Section;

    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
        steamgriddb::Secret::new("private-value".to_owned()),
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
    let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
        generation,
        sequence,
        input_session,
        Err("SteamGridDB rejected the key".to_owned()),
    )));
    let _ = app.update(Message::Section(Section::Settings));

    let settings = &app.settings.as_ref().unwrap().steamgriddb;
    assert!(settings.input_open);
    assert!(settings.input.as_str().is_empty());
    assert_eq!(
        settings.error.as_deref(),
        Some("SteamGridDB rejected the key")
    );
}

#[test]
fn completed_save_drops_an_older_status_result() {
    use crate::sidebar::Section;

    let (_root, paths) = setup();
    let config = paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "schema_version = 1\nkind = 'native'\n").unwrap();

    let mut app = open_global_app(&paths);
    let old_generation = app.settings.as_ref().unwrap().generation;
    let old_status_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    let identity = crate::art_page::EnvironmentIdentity::read(&paths, "game").unwrap();
    app.art_pages.insert(
        "game".to_owned(),
        crate::art_page::Page::new("game".to_owned(), 1, identity),
    );
    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Section(Section::Settings));
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
        steamgriddb::Secret::new("private-value".to_owned()),
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
    let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
        generation,
        sequence,
        input_session,
        Ok(steamgriddb::Saved {
            status: steamgriddb::Status {
                source: None,
                override_note: false,
            },
            warning: None,
            result: Some("Saved the SteamGridDB key".to_owned()),
        }),
    )));

    let refreshed_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
        generation,
        refreshed_sequence,
        Ok(steamgriddb::Status {
            source: Some(wineo_core::steamgriddb::KeySource::Keyring),
            override_note: false,
        }),
    )));
    let _ = app.update(Message::SteamGridDbKeyChangedAt(refreshed_sequence, true));

    let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
        old_generation,
        old_status_sequence,
        Ok(steamgriddb::Status {
            source: None,
            override_note: false,
        }),
    )));

    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .steamgriddb
            .status
            .as_ref()
            .unwrap()
            .source,
        Some(wineo_core::steamgriddb::KeySource::Keyring)
    );
    assert!(app.steamgriddb_available);
}

#[test]
fn newest_pending_status_can_land_after_reopening_settings() {
    use crate::sidebar::Section;

    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let old_generation = app.settings.as_ref().unwrap().generation;
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

    let _ = app.update(Message::Section(Section::Trash));
    let _ = app.update(Message::Section(Section::Settings));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
        old_generation,
        sequence,
        Ok(steamgriddb::Status {
            source: None,
            override_note: false,
        }),
    )));

    let page = app.settings.as_ref().unwrap();
    assert_ne!(page.generation, old_generation);
    assert_eq!(page.steamgriddb.status.as_ref().unwrap().source, None);
}

#[test]
fn old_key_operation_results_leave_a_new_input_session_untouched() {
    use crate::sidebar::Section;

    for remove in [false, true] {
        let (_root, paths) = setup();
        let mut app = open_global_app(&paths);
        let old_generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("old-input".to_owned()),
        )));
        let old_input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
        let operation = if remove {
            Event::SteamGridDbKeyRemove
        } else {
            Event::SteamGridDbKeySet
        };
        let _ = app.update(Message::Settings(operation));
        let old_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;

        let _ = app.update(Message::Section(Section::Trash));
        let _ = app.update(Message::Section(Section::Settings));
        let new_generation = app.settings.as_ref().unwrap().generation;
        let _ = app.update(Message::Settings(Event::Loaded(
            new_generation,
            load::snapshot_global(&paths),
        )));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
        let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
            steamgriddb::Secret::new("new-input".to_owned()),
        )));
        let new_input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;

        let result = if remove {
            Event::SteamGridDbKeyRemoved(
                old_generation,
                old_sequence,
                old_input_session,
                Ok(steamgriddb::Removed {
                    status: steamgriddb::Status {
                        source: None,
                        override_note: false,
                    },
                    result: "Old remove completed".to_owned(),
                }),
            )
        } else {
            Event::SteamGridDbKeySaved(
                old_generation,
                old_sequence,
                old_input_session,
                Ok(steamgriddb::Saved {
                    status: steamgriddb::Status {
                        source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                        override_note: false,
                    },
                    warning: None,
                    result: Some("Old save completed".to_owned()),
                }),
            )
        };
        let _ = app.update(Message::Settings(result));

        let settings = &app.settings.as_ref().unwrap().steamgriddb;
        assert!(settings.input_open);
        assert_eq!(settings.input.as_str(), "new-input");
        assert_eq!(
            app.settings.as_ref().unwrap().steamgriddb_input_session,
            new_input_session
        );
        assert_eq!(
            settings.result.as_deref(),
            Some(if remove {
                "Old remove completed"
            } else {
                "Old save completed"
            })
        );
    }
}

#[test]
fn key_work_allows_saving_unrelated_global_edits() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let generation = app.settings.as_ref().unwrap().generation;
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySetOpen));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyText(
        steamgriddb::Secret::new("private-value".to_owned()),
    )));
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySet));
    let input_session = app.settings.as_ref().unwrap().steamgriddb_input_session;
    let sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    assert!(app.settings.as_ref().unwrap().can_save());

    let _ = app.update(Message::Settings(Event::Save));

    assert!(app.settings.as_ref().unwrap().saving);
    assert!(!app.settings.as_ref().unwrap().can_save());
    let _ = app.update(Message::Settings(Event::SteamGridDbKeySaved(
        generation,
        sequence,
        input_session,
        Ok(steamgriddb::Saved {
            status: steamgriddb::Status {
                source: Some(wineo_core::steamgriddb::KeySource::Keyring),
                override_note: false,
            },
            warning: None,
            result: None,
        }),
    )));
    let status_sequence = app.settings.as_ref().unwrap().steamgriddb_sequence;
    let _ = app.update(Message::Settings(Event::SteamGridDbKeyStatusLoaded(
        generation,
        status_sequence,
        Ok(steamgriddb::Status {
            source: Some(wineo_core::steamgriddb::KeySource::Keyring),
            override_note: false,
        }),
    )));
    assert!(app.settings.as_ref().unwrap().saving);
    assert!(app.settings.as_ref().unwrap().is_dirty());
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .steamgriddb
            .status
            .as_ref()
            .unwrap()
            .source,
        Some(wineo_core::steamgriddb::KeySource::Keyring)
    );
}

#[test]
fn save_key_reaches_both_settings_pages_without_the_library_search_widget() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    for global in [false, true] {
        let mut app = if global {
            let mut app = App::boot(paths.clone()).0;
            app.settings = Some(global_page(&paths));
            app
        } else {
            open_app(&paths)
        };
        let event = if global {
            Event::GlobalSwitch(Switch::Gamemode, true)
        } else {
            Event::Switch(Switch::Wayland, true)
        };
        let _ = app.update(Message::Settings(event));
        let _ = app.update(Message::Key(
            Key::Character("s".into()),
            Modifiers::CTRL,
            iced::event::Status::Captured,
        ));
        assert!(app.settings.as_ref().unwrap().saving);
    }
}

#[test]
fn escape_leaves_environment_settings_but_not_global_settings() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    for global in [false, true] {
        let mut app = if global {
            let mut app = App::boot(paths.clone()).0;
            app.settings = Some(global_page(&paths));
            app
        } else {
            open_app(&paths)
        };
        let _ = app.update(Message::Key(
            Key::Named(Named::Escape),
            Modifiers::empty(),
            iced::event::Status::Captured,
        ));
        assert!(app.settings.is_some());
        let _ = app.update(Message::Key(
            Key::Named(Named::Escape),
            Modifiers::empty(),
            iced::event::Status::Ignored,
        ));
        assert_eq!(app.settings.is_some(), global);
    }
}

#[test]
fn mouse_back_leaves_environment_settings_even_when_captured() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = open_app(&paths);
    crate::tests::press_back(&mut app, iced::event::Status::Captured);
    assert!(app.settings.is_none());
}

#[test]
fn escape_asks_before_leaving_and_again_keeps_editing() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = open_app(&paths);
    let escape = Key::Named(Named::Escape);
    // A focused field takes the first Escape.
    let _ = app.settings_key(&escape, Modifiers::empty(), true);
    assert!(!app.settings.as_ref().unwrap().leave_prompt);
    let _ = app.update(Message::Settings(Event::Switch(Switch::Wayland, true)));
    let _ = app.settings_key(&escape, Modifiers::empty(), false);
    assert!(app.settings.as_ref().unwrap().leave_prompt);
    let _ = app.settings_key(&escape, Modifiers::empty(), false);
    let page = app.settings.as_ref().unwrap();
    assert!(!page.leave_prompt && page.is_dirty());
    let _ = app.settings_key(&Key::Character("s".into()), Modifiers::CTRL, true);
    assert!(app.settings.as_ref().unwrap().saving);
}

#[test]
fn escape_with_a_menu_open_stays_on_the_page() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = open_app(&paths);
    let escape = Key::Named(Named::Escape);
    let _ = app.update(Message::Settings(Event::MenuOpened));
    let _ = app.settings_key(&escape, Modifiers::empty(), false);
    assert!(app.settings.is_some());
    let _ = app.update(Message::Settings(Event::MenuClosed));
    let _ = app.settings_key(&escape, Modifiers::empty(), false);
    assert!(app.settings.is_none());
}

#[test]
fn leaving_a_clean_page_returns_to_the_library() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = open_app(&paths);
    let _ = app.update(Message::Settings(Event::Back));
    assert!(app.settings.is_none());
    assert_eq!(app.selected.as_deref(), Some("game"));
}

fn sidebar_app(paths: &Paths, global: bool) -> App {
    if global {
        let mut app = App::boot(paths.clone()).0;
        app.settings = Some(global_page(paths));
        app
    } else {
        open_app(paths)
    }
}

fn sidebar_request(section: crate::sidebar::Section, keyboard: bool) -> Message {
    if keyboard {
        let digit = match section {
            crate::sidebar::Section::Library => "1",
            crate::sidebar::Section::Trash => "2",
            crate::sidebar::Section::Checkup => "3",
            crate::sidebar::Section::Downloads => "4",
            crate::sidebar::Section::Snapshots => "5",
            crate::sidebar::Section::Settings => ",",
        };
        let (mut messages, status) =
            crate::shortcuts::tests::input_messages("settings field", digit, Modifiers::CTRL);
        assert_eq!(status, iced::event::Status::Captured);
        assert!(matches!(messages.as_slice(), [Message::Section(_)]));
        messages.pop().unwrap()
    } else {
        Message::Section(section)
    }
}

#[test]
fn sidebar_leaves_clean_settings_and_confirms_dirty_settings() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    for global in [false, true] {
        for keyboard in [false, true] {
            for section in [
                Section::Library,
                Section::Trash,
                Section::Checkup,
                Section::Downloads,
            ] {
                let mut app = sidebar_app(&paths, global);
                let _ = app.update(sidebar_request(section, keyboard));
                assert!(app.settings.is_none());
                assert_eq!(app.section, section);

                let mut app = sidebar_app(&paths, global);
                let edit = if global {
                    Event::GlobalSwitch(Switch::Gamemode, true)
                } else {
                    Event::Switch(Switch::Wayland, true)
                };
                let _ = app.update(Message::Settings(edit));
                let _ = app.update(sidebar_request(section, keyboard));
                let page = app.settings.as_ref().unwrap();
                assert!(page.is_dirty() && page.leave_prompt);
                assert_eq!(app.section, Section::Library);
                assert_eq!(app.pending_section, Some(section));
                let _ = app.update(Message::Settings(Event::Discard));
                assert!(app.settings.is_none());
                assert_eq!(app.section, section);
                assert_eq!(app.pending_section, None);
            }
        }
    }
}

#[test]
fn checkup_heading_opens_details_after_discard_and_forgets_kept_edits() {
    let (_root, paths) = setup();
    write(&paths, GAME);
    std::fs::write(paths.environment_config("other"), "kind = 'native'\n").unwrap();
    for global in [false, true] {
        for discard in [false, true] {
            let mut app = sidebar_app(&paths, global);
            app.cards = crate::library::load_library(&paths).unwrap();
            app.selected = Some("other".into());
            let edit = if global {
                Event::GlobalSwitch(Switch::Gamemode, true)
            } else {
                Event::Switch(Switch::Wayland, true)
            };
            let _ = app.update(Message::Settings(edit));
            let _ = app.update(Message::Checkup(crate::checkup::Event::Environment(
                "game".into(),
            )));
            assert!(app.settings.as_ref().unwrap().leave_prompt);
            assert_eq!(app.selected.as_deref(), Some("other"));
            assert_eq!(app.pending_checkup_environment.as_deref(), Some("game"));
            if discard {
                let _ = app.update(Message::Settings(Event::Discard));
                assert!(app.settings.is_none() && app.panel_open);
                assert_eq!(app.selected.as_deref(), Some("game"));
                assert_eq!(app.section, crate::sidebar::Section::Library);
            } else {
                let _ = app.update(Message::Settings(Event::KeepEditing));
                assert!(app.settings.as_ref().unwrap().is_dirty());
                assert_eq!(app.selected.as_deref(), Some("other"));
                assert!(app.pending_checkup_environment.is_none());
                let _ = app.update(Message::Settings(Event::Back));
                let _ = app.update(Message::Settings(Event::Discard));
                assert_eq!(app.selected.as_deref(), Some("other"));
            }
            assert!(app.pending_section.is_none());
            assert!(app.pending_checkup_environment.is_none());
        }
    }
}

#[test]
fn sidebar_keys_respect_fields_and_the_unsaved_prompt() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    for global in [false, true] {
        let mut app = sidebar_app(&paths, global);
        let edit = if global {
            Event::GlobalSwitch(Switch::Gamemode, true)
        } else {
            Event::Switch(Switch::Wayland, true)
        };
        let _ = app.update(Message::Settings(edit));
        for digit in ["1", "2", "3"] {
            let _ = app.update(Message::Key(
                Key::Character(digit.into()),
                Modifiers::CTRL,
                iced::event::Status::Captured,
            ));
            assert!(!app.settings.as_ref().unwrap().leave_prompt);
            assert_eq!(app.pending_section, None);
        }
        let _ = app.update(sidebar_request(Section::Trash, true));
        assert!(app.settings.as_ref().unwrap().leave_prompt);
        assert_eq!(app.pending_section, Some(Section::Trash));
        for section in [Section::Library, Section::Checkup] {
            let _ = app.update(sidebar_request(section, true));
            let _ = app.update(Message::Section(section));
            let _ = app.update(Message::KeyWithFocus(
                Key::Character("3".into()),
                Modifiers::CTRL,
                iced::event::Status::Ignored,
                false,
            ));
            assert_eq!(app.section, Section::Library);
            assert_eq!(app.pending_section, Some(Section::Trash));
        }
        let _ = app.update(Message::Settings(Event::KeepEditing));
        assert!(app.settings.as_ref().unwrap().is_dirty());
        assert!(!app.settings.as_ref().unwrap().leave_prompt);
        assert_eq!(app.pending_section, None);
        let _ = app.update(Message::Settings(Event::Back));
        let _ = app.update(Message::Settings(Event::Discard));
        assert_eq!(app.section, Section::Library);
    }
}

#[test]
fn sidebar_cannot_leave_settings_during_a_save_or_open_menu() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    for global in [false, true] {
        let mut app = sidebar_app(&paths, global);
        let _ = app.update(Message::Settings(Event::MenuOpened));
        let _ = app.update(sidebar_request(Section::Trash, true));
        let _ = app.update(Message::Section(Section::Trash));
        assert!(app.settings.is_some());
        assert_eq!(app.section, Section::Library);
        assert_eq!(app.pending_section, None);
        let _ = app.update(Message::Settings(Event::MenuClosed));
        app.settings.as_mut().unwrap().saving = true;
        let _ = app.update(sidebar_request(Section::Checkup, true));
        let _ = app.update(Message::Section(Section::Checkup));
        assert!(app.settings.is_some());
        assert_eq!(app.section, Section::Library);
        assert_eq!(app.pending_section, None);
    }
}

#[test]
fn settings_row_is_selected_only_for_global_settings() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    let app = App::boot(paths.clone()).0;
    assert!(app.section_selected(Section::Library));
    assert!(!app.section_selected(Section::Settings));

    let app = sidebar_app(&paths, true);
    assert!(app.section_selected(Section::Settings));
    assert!(!app.section_selected(Section::Library));
    assert!(!app.section_selected(Section::Trash));

    let app = sidebar_app(&paths, false);
    assert!(app.section_selected(Section::Library));
    assert!(!app.section_selected(Section::Settings));
}

#[test]
fn control_comma_and_the_row_open_global_settings_from_any_section() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    for section in [
        Section::Library,
        Section::Trash,
        Section::Checkup,
        Section::Downloads,
    ] {
        for keyboard in [false, true] {
            let mut app = App::boot(paths.clone()).0;
            let _ = app.update(Message::Section(section));
            let _ = app.update(sidebar_request(Section::Settings, keyboard));
            assert!(app.global_settings_open());
            assert_eq!(app.section, Section::Library);
            assert!(app.section_selected(Section::Settings));
        }
    }
    let mut app = App::boot(paths.clone()).0;
    crate::shortcuts::tests::press(&mut app, ",", Modifiers::CTRL);
    assert!(app.global_settings_open());
}

#[test]
fn opening_settings_again_keeps_global_edits_and_its_page() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = sidebar_app(&paths, true);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    let generation = app.settings.as_ref().unwrap().generation;
    for keyboard in [false, true] {
        let _ = app.update(sidebar_request(Section::Settings, keyboard));
        let page = app.settings.as_ref().unwrap();
        assert!(page.is_dirty() && !page.leave_prompt);
        assert_eq!(page.generation, generation);
        assert_eq!(app.pending_section, None);
    }
}

#[test]
fn environment_settings_with_edits_prompt_before_opening_global_settings() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    for keyboard in [false, true] {
        let mut app = sidebar_app(&paths, false);
        let _ = app.update(Message::Settings(Event::Switch(Switch::Wayland, true)));
        let _ = app.update(sidebar_request(Section::Settings, keyboard));
        let page = app.settings.as_ref().unwrap();
        assert!(page.is_dirty() && page.leave_prompt && !page.is_global());
        assert_eq!(app.pending_section, Some(Section::Settings));
        assert!(app.section_selected(Section::Library));

        let _ = app.update(Message::Settings(Event::KeepEditing));
        assert!(!app.settings.as_ref().unwrap().is_global());
        assert_eq!(app.pending_section, None);

        let _ = app.update(sidebar_request(Section::Settings, keyboard));
        let _ = app.update(Message::Settings(Event::Discard));
        assert!(app.global_settings_open());
        assert_eq!(app.pending_section, None);
        assert!(app.section_selected(Section::Settings));
    }
    let mut app = sidebar_app(&paths, false);
    let _ = app.update(Message::Section(Section::Settings));
    assert!(app.global_settings_open());
}

#[test]
fn settings_row_waits_for_a_save_or_open_menu() {
    use crate::sidebar::Section;
    let (_root, paths) = setup();
    write(&paths, GAME);
    let mut app = sidebar_app(&paths, false);
    let _ = app.update(Message::Settings(Event::MenuOpened));
    let _ = app.update(sidebar_request(Section::Settings, true));
    let _ = app.update(Message::Section(Section::Settings));
    assert!(!app.global_settings_open());
    assert_eq!(app.pending_section, None);
}

fn target(page: &mut Page, paths: &Paths, name: &str, edit: TargetEdit) {
    let _ = page.update(paths, Event::Target(name.into(), edit));
}

/// Sends Add and lands the check the way the worker would.
fn add(page: &mut Page, paths: &Paths) {
    let path = editor(page).new_target.path.clone();
    let _ = page.update(paths, Event::TargetAdd);
    assert!(page.resolving());
    let result = load::resolve_exe(paths, "game", &path);
    let sequence = page.resolve_sequence;
    let _ = page.update(paths, Event::Resolved(1, sequence, result));
}

const TARGETED: &str = r#"# my game
schema_version = 1
prefix = "~/prefixes/game"
runner = "missing" # pinned
arch = "win64"

# the main one
[targets.game]
exe = 'C:\Games\game.exe' # launcher
args = ["-a"] # flags
default = true # starts first

# a helper
[targets.tool]
exe = "/usr/bin/tool" # tool
cwd = "/tmp" # scratch
name = "Tool" # entry
desktop = true # shown

# going away
[targets.old]
exe = "/usr/bin/old"
"#;

#[test]
fn existing_target_paths_are_cleaned_when_edited() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let mut page = page(&paths);
    target(
        &mut page,
        &paths,
        "tool",
        TargetEdit::Exe(" '/opt/My App/app.exe' ".into()),
    );
    target(
        &mut page,
        &paths,
        "tool",
        TargetEdit::Cwd(r"/opt/My\ App".into()),
    );
    let values = &editor(&page).form.draft.targets["tool"];
    assert_eq!(values.exe, "/opt/My App/app.exe");
    assert_eq!(values.cwd, "/opt/My App");
    let edits = editor(&page).form.edits();
    assert_eq!(
        edits.pairs,
        [
            ("targets.tool.exe".into(), "/opt/My App/app.exe".into()),
            ("targets.tool.cwd".into(), "/opt/My App".into()),
        ]
    );
    let _ = page.update(&paths, Event::Revert);
    assert_eq!(
        editor(&page).form.draft.targets["tool"].exe,
        "/usr/bin/tool"
    );
    assert!(!page.is_dirty());
}

#[test]
fn saving_target_edits_changes_only_those_lines() {
    let (root, paths) = setup();
    write(&paths, TARGETED);
    let extra = root.path().join("Apps/extra.exe");
    fs::create_dir_all(extra.parent().unwrap()).unwrap();
    fs::write(&extra, "").unwrap();
    let mut page = page(&paths);
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::ArgsInput("-b".into()),
    );
    target(&mut page, &paths, "game", TargetEdit::ArgsAdd);
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Cwd(r"C:\Games".into()),
    );
    target(&mut page, &paths, "game", TargetEdit::Name("Game".into()));
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Switch(Switch::Gamemode, false),
    );
    target(&mut page, &paths, "game", TargetEdit::Gamescope(false));
    target(
        &mut page,
        &paths,
        "tool",
        TargetEdit::Exe("/usr/bin/tool2".into()),
    );
    target(&mut page, &paths, "tool", TargetEdit::Desktop(false));
    target(&mut page, &paths, "tool", TargetEdit::Default(true));
    target(&mut page, &paths, "old", TargetEdit::AskRemove);
    target(&mut page, &paths, "old", TargetEdit::Remove);
    let _ = page.update(&paths, Event::AddOpen(true));
    let _ = page.update(
        &paths,
        Event::NewTargetPath(format!("\"{}\"", extra.display())),
    );
    add(&mut page, &paths);
    let shown = editor(&page);
    assert_eq!(shown.add_error, None);
    assert!(!shown.add_open && shown.open_cards.contains("extra"));
    assert_eq!(page.changes(), 10);
    assert!(page.can_save(), "{:?}", page.problem());
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert_eq!(
        text,
        r#"# my game
schema_version = 1
prefix = "~/prefixes/game"
runner = "missing" # pinned
arch = "win64"

# the main one
[targets.game]
exe = 'C:\Games\game.exe' # launcher
args = ["-a", "-b"] # flags
cwd = 'C:\Games'
name = "Game"
gamemode = false
gamescope = false

# a helper
[targets.tool]
exe = "/usr/bin/tool2" # tool
cwd = "/tmp" # scratch
name = "Tool" # entry
default = true

[targets.extra]
exe = "~/Apps/extra.exe"
name = "extra"
desktop = true
"#
    );
    saved(&mut page, &paths, &[], None);
    land(&mut page, &paths);
    assert!(!page.is_dirty(), "{:?}", editor(&page).form.edits());
    assert!(editor(&page).open_cards.contains("extra"));
}

#[test]
fn removing_a_target_asks_first_and_shows_its_art_and_entry() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let art = wineo_core::art::target_dir(&paths.art_dir("game"), "tool");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("icon.png"), "").unwrap();
    let mut page = page(&paths);
    assert_eq!(
        editor(&page).info.target_art["tool"],
        "~/.local/share/wineo/art/game/tool"
    );
    assert!(!editor(&page).info.target_art.contains_key("game"));
    // Remove without asking does nothing.
    target(&mut page, &paths, "tool", TargetEdit::Remove);
    assert!(!page.is_dirty());
    target(&mut page, &paths, "tool", TargetEdit::AskRemove);
    let _ = page.update(&paths, Event::KeepTarget);
    target(&mut page, &paths, "tool", TargetEdit::Remove);
    assert!(!page.is_dirty());
    target(&mut page, &paths, "tool", TargetEdit::AskRemove);
    assert_eq!(editor(&page).removing.as_deref(), Some("tool"));
    target(&mut page, &paths, "tool", TargetEdit::Remove);
    assert_eq!(editor(&page).form.edits().unset, ["targets.tool"]);
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    // Core cleans up the art of a target a save removes.
    assert!(!art.exists());
}

#[test]
fn a_target_follows_the_environment_draft_live() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    write_global(&paths, "mangohud = true\n");
    let mut page = page(&paths);
    let label = |page: &Page, switch| {
        let shown = editor(page);
        shown
            .form
            .target_switch("game", switch, &shown.info.inherited)
    };
    assert_eq!(label(&page, Switch::Mangohud), (true, Source::Global));
    let _ = page.update(&paths, Event::Switch(Switch::Mangohud, false));
    assert_eq!(label(&page, Switch::Mangohud), (false, Source::Environment));
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Switch(Switch::Mangohud, true),
    );
    assert_eq!(label(&page, Switch::Mangohud), (true, Source::Here));
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Reset(Switch::Mangohud),
    );
    assert_eq!(label(&page, Switch::Mangohud), (false, Source::Environment));
    // Editing gamescope while inheriting copies the environment's draft.
    let _ = page.update(&paths, Event::Gamescope(true));
    let _ = page.update(&paths, Event::FieldText(Field::Width, "1280".into()));
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::FieldText(Field::Height, "720".into()),
    );
    let fields = &editor(&page).form.draft.targets["game"].fields;
    assert_eq!(
        (fields.width.as_str(), fields.height.as_str()),
        ("1280", "720")
    );
    assert_eq!(
        editor(&page).form.target_gamescope("game"),
        (true, Source::Here)
    );
    target(&mut page, &paths, "game", TargetEdit::GamescopeReset);
    target(&mut page, &paths, "tool", TargetEdit::GamescopeHere);
    let tool = &editor(&page).form.draft.targets["tool"];
    assert_eq!(tool.gamescope, form::TargetGamescope::On);
    assert_eq!(tool.fields.width, "1280");
}

#[test]
fn typed_target_text_survives_a_quiet_reload_and_revert_clears_it() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::AddOpen(true));
    let _ = page.update(&paths, Event::NewTargetName("new".into()));
    let _ = page.update(&paths, Event::NewTargetPath("/bin/new".into()));
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::ArgsInput("-x".into()),
    );
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::GamescopeArgsInput("-f".into()),
    );
    let _ = page.update(&paths, Event::Card("game".into(), true));
    write(&paths, &TARGETED.replace("[\"-a\"]", "[\"-z\"]"));
    check(&mut page, &paths);
    land(&mut page, &paths);
    let shown = editor(&page);
    assert_eq!(shown.form.draft.targets["game"].args, ["-z"]);
    assert_eq!(
        (
            shown.new_target.name.as_str(),
            shown.new_target.path.as_str()
        ),
        ("new", "/bin/new")
    );
    assert_eq!(shown.target_args_input("game"), "-x");
    assert_eq!(shown.target_gamescope_args_input("game"), "-f");
    assert!(shown.add_open && shown.open_cards.contains("game"));
    target(&mut page, &paths, "game", TargetEdit::ArgsAdd);
    assert_eq!(editor(&page).form.draft.targets["game"].args, ["-z", "-x"]);
    let _ = page.update(&paths, Event::Revert);
    let shown = editor(&page);
    assert_eq!(shown.new_target, NewTarget::default());
    assert_eq!(shown.target_gamescope_args_input("game"), "");
    assert!(!page.is_dirty());
}

#[test]
fn adding_a_target_checks_the_path_and_the_name() {
    let (root, paths) = setup();
    write(&paths, TARGETED);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::NewTargetName("tool".into()));
    let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
    assert!(!editor(&page).can_add_target());
    let _ = page.update(&paths, Event::TargetAdd);
    assert!(!page.resolving());
    let _ = page.update(&paths, Event::NewTargetName("sh".into()));
    for (typed, error) in [
        ("bin/sh", "The path must be absolute or start with ~/."),
        ("/nowhere/x.exe", "`/nowhere/x.exe` does not exist."),
        (r"C:\x.exe", r"`C:\x.exe` does not exist."),
    ] {
        let _ = page.update(&paths, Event::NewTargetPath(typed.into()));
        add(&mut page, &paths);
        assert_eq!(editor(&page).add_error.as_deref(), Some(error), "{typed}");
    }
    assert!(!page.is_dirty());
    // A check that lands after the form changed is dropped, and so is
    // one that isn't the latest.
    let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
    let _ = page.update(&paths, Event::TargetAdd);
    let _ = page.update(&paths, Event::NewTargetName("shell".into()));
    let sequence = page.resolve_sequence;
    let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
    assert!(!page.is_dirty() && !page.resolving());
    let _ = page.update(&paths, Event::TargetAdd);
    let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
    assert!(!page.is_dirty() && page.resolving());
    // One that lands while a save is locking the page is dropped too.
    let sequence = page.resolve_sequence;
    page.saving = true;
    let _ = page.update(&paths, Event::Resolved(1, sequence, Ok("/bin/sh".into())));
    page.saving = false;
    assert!(!page.is_dirty() && !page.resolving());
    // An empty name takes one suggested from the exe.
    let exe = root.path().join("Apps/My Tool.exe");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, "").unwrap();
    let _ = page.update(&paths, Event::NewTargetName(String::new()));
    let _ = page.update(
        &paths,
        Event::NewTargetPath(exe.display().to_string().replace(' ', "\\ ")),
    );
    add(&mut page, &paths);
    let added = &editor(&page).form.draft.targets["my-tool"];
    assert_eq!(added.exe, "~/Apps/My Tool.exe");
    assert!(added.desktop);
    assert_eq!(added.name, "My Tool");
}

#[test]
fn add_target_notes_an_exe_already_used_in_the_draft() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let prefix = fake_prefix(&paths);
    let game = prefix.join("drive_c/Games/game.exe");
    fs::write(&game, "").unwrap();
    let mut page = page(&paths);
    let _ = page.update(
        &paths,
        Event::ProgramPicked(ProgramChoice {
            name: "Game Again".into(),
            exe: r"c:\games\GAME.exe".into(),
        }),
    );
    assert_eq!(editor(&page).duplicate_target(), Some("game"));
    assert!(editor(&page).can_add_target());
    let _ = page.update(&paths, Event::NewTargetPath(game.display().to_string()));
    assert_eq!(editor(&page).duplicate_target(), Some("game"));
    target(&mut page, &paths, "tool", TargetEdit::Exe("/bin/sh".into()));
    let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
    assert_eq!(editor(&page).duplicate_target(), Some("tool"));
    assert!(editor(&page).can_add_target());
}

fn fake_prefix(paths: &Paths) -> std::path::PathBuf {
    let prefix = paths.expand("~/prefixes/game");
    fs::create_dir_all(prefix.join("dosdevices")).unwrap();
    fs::write(prefix.join("system.reg"), "").unwrap();
    std::os::unix::fs::symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
    for exe in [
        "drive_c/Games/Hades/Hades.exe",
        "drive_c/Games/Hades/unins000.exe",
    ] {
        let path = prefix.join(exe);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    prefix
}

#[test]
fn installed_programs_are_offered_once_the_prefix_exists() {
    let (_root, paths) = setup();
    write(&paths, TARGETED);
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::AddOpen(true));
    let sequence = page.scan_sequence;
    assert_eq!(editor(&page).scan, Some(Scan::Scanning));
    let _ = page.update(
        &paths,
        Event::Scanned(1, sequence, load::scan(&paths, "game")),
    );
    assert_eq!(editor(&page).scan, Some(Scan::NoPrefix));
    let prefix = fake_prefix(&paths);
    let shortcut =
        prefix.join("drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs/Broken.lnk");
    fs::create_dir_all(shortcut.parent().unwrap()).unwrap();
    fs::write(shortcut, "not a shortcut").unwrap();
    let _ = page.update(
        &paths,
        Event::Scanned(1, sequence, load::scan(&paths, "game")),
    );
    let current = editor(&page).scan.clone();
    let _ = page.update(
        &paths,
        Event::Scanned(7, sequence, Scan::Failed("stale".into())),
    );
    assert_eq!(editor(&page).scan, current);
    let Some(Scan::Programs {
        programs,
        held_back,
        unreadable,
    }) = editor(&page).scan.clone()
    else {
        panic!("{:?}", editor(&page).scan);
    };
    assert_eq!(
        programs,
        [ProgramChoice {
            name: "Hades".into(),
            exe: r"C:\Games\Hades\Hades.exe".into(),
        }]
    );
    assert_eq!(held_back.len(), 1);
    assert_eq!(unreadable, ["Skipped the shortcut Broken: not a shortcut"]);
    let _ = page.update(&paths, Event::ProgramPicked(programs[0].clone()));
    assert_eq!(
        editor(&page).new_target,
        NewTarget {
            name: "hades".into(),
            path: r"C:\Games\Hades\Hades.exe".into(),
            label: Some("Hades".into()),
        }
    );
    add(&mut page, &paths);
    assert_eq!(
        editor(&page).form.draft.targets["hades"].exe,
        r"C:\Games\Hades\Hades.exe"
    );
    // A Unix path inside drive_c is stored as its C: path.
    let _ = page.update(&paths, Event::NewTargetName("uninstall".into()));
    let unins = prefix.join("drive_c/Games/Hades/unins000.exe");
    let _ = page.update(&paths, Event::NewTargetPath(unins.display().to_string()));
    add(&mut page, &paths);
    let added = &editor(&page).form.draft.targets["uninstall"];
    assert_eq!(added.exe, r"C:\Games\Hades\unins000.exe");
    assert_eq!(added.name, "unins000");
}

#[test]
fn a_native_page_adds_targets_by_path_only() {
    let (root, paths) = setup();
    let bin = root.path().join("bin");
    executable(&bin.join("dusk"));
    write(
        &paths,
        &format!(
            "schema_version = 1\nkind = \"native\"\n[env]\nPATH = \"{}\"\n[targets.game]\nexe = \"dusk\"\n",
            bin.display()
        ),
    );
    let mut page = page(&paths);
    let _ = page.update(&paths, Event::AddOpen(true));
    assert_eq!(editor(&page).scan, None);
    let _ = page.update(
        &paths,
        Event::ProgramPicked(ProgramChoice {
            name: "X".into(),
            exe: r"C:\x.exe".into(),
        }),
    );
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Switch(Switch::Wayland, true),
    );
    target(
        &mut page,
        &paths,
        "game",
        TargetEdit::Reset(Switch::Wayland),
    );
    assert!(!page.is_dirty());
    assert_eq!(editor(&page).new_target, NewTarget::default());
    let _ = page.update(&paths, Event::NewTargetName("win".into()));
    let _ = page.update(&paths, Event::NewTargetPath(r"C:\x.exe".into()));
    add(&mut page, &paths);
    assert!(
        editor(&page)
            .add_error
            .as_deref()
            .is_some_and(|error| error.contains("native")),
        "{:?}",
        editor(&page).add_error
    );
    // A bare name is looked up on the environment's PATH.
    let _ = page.update(&paths, Event::NewTargetName("dusk".into()));
    let _ = page.update(&paths, Event::NewTargetPath("dusk".into()));
    add(&mut page, &paths);
    assert_eq!(editor(&page).form.draft.targets["dusk"].exe, "dusk");
    target(&mut page, &paths, "game", TargetEdit::Default(true));
    let applied = apply(&page, &paths);
    assert!(applied.failed.is_none(), "{:?}", applied.failed);
    let text = fs::read_to_string(paths.environment_config("game")).unwrap();
    assert!(text.contains("[targets.dusk]\nexe = \"dusk\""), "{text}");
}

#[test]
fn a_portable_page_refuses_desktop_entries() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let portable = root.path().join("portable");
    fs::create_dir_all(&portable).unwrap();
    let paths = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&portable),
    )
    .unwrap();
    write(
        &paths,
        "schema_version = 1\nprefix = \"prefixes/game\"\nrunner = \"missing\"\narch = \"win64\"\n[targets.game]\nexe = \"/bin/sh\"\ndesktop = true\n[targets.tool]\nexe = \"/bin/true\"\ndefault = true\n",
    );
    let mut page = page(&paths);
    let shown = editor(&page);
    assert_eq!(
        shown.desktop_refusal().as_deref(),
        Some("desktop entries aren't supported for portable environments")
    );
    target(&mut page, &paths, "tool", TargetEdit::Desktop(true));
    assert!(!page.is_dirty());
    // A hand-written one can still be turned off.
    target(&mut page, &paths, "game", TargetEdit::Desktop(false));
    assert_eq!(editor(&page).form.edits().unset, ["targets.game.desktop"]);
    // New targets get no entry, and no name to title one.
    let _ = page.update(&paths, Event::NewTargetName("sh".into()));
    let _ = page.update(&paths, Event::NewTargetPath("/bin/sh".into()));
    add(&mut page, &paths);
    let added = &editor(&page).form.draft.targets["sh"];
    assert!(!added.desktop && added.name.is_empty());
    let relative = portable.join("apps/other.exe");
    fs::create_dir_all(relative.parent().unwrap()).unwrap();
    fs::write(relative, "").unwrap();
    let _ = page.update(&paths, Event::NewTargetName("other".into()));
    let _ = page.update(&paths, Event::NewTargetPath("apps/other.exe".into()));
    add(&mut page, &paths);
    assert_eq!(editor(&page).add_error, None);
    assert_eq!(
        editor(&page).form.draft.targets["other"].exe,
        "apps/other.exe"
    );
}

#[test]
fn a_save_that_fell_back_to_default_settings_says_so() {
    let applied = settings::Applied {
        warnings: vec!["using default settings: invalid config /c.toml:\nbad".to_owned()],
        desktop: vec![wineo_core::desktop::Change::Warning(
            "again\nlater".to_owned(),
        )],
        ..Default::default()
    };
    let report = load::report("hades", applied);
    assert_eq!(
        report.lines,
        [
            "Warning: using default settings: invalid config /c.toml: bad",
            "Warning: again later"
        ]
    );
}

mod tabs;

mod appearance_tab;
