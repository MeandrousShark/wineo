use crate::test_script::write_script;
use std::ffi::OsString;

use super::*;
use wineo_core::config::TITLE_MAX_CHARS;

fn page() -> Page {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    Page::new(paths).0
}

#[test]
fn installer_log_stays_capped_and_mirrored_in_the_view() {
    let mut page = page();
    for index in 0..4000 {
        let _ = page.append_log(&format!("{index} {}\r\n", "x".repeat(60)));
    }
    assert!(page.log.len() <= LOG_CAP);
    assert!(page.log.len() > LOG_KEEP);
    assert!(!page.log.contains('\r'));
    assert_eq!(page.log_view.content().text(), page.log);
}

#[test]
fn installer_log_pauses_while_selected_and_copies_everything() {
    let mut page = page();
    let _ = page.append_log("first\n");
    let _ = page.update(Event::LogAction(
        iced::widget::text_editor::Action::SelectAll,
    ));
    let _ = page.append_log("second\n");
    assert!(page.log_view.paused());
    assert_eq!(page.log_view.content().text(), "first\n");
    let _ = page.update(Event::CopyLog);
    assert!(page.log_view.copied());
    assert_eq!(page.log, "first\nsecond\n");
    let _ = page.update(Event::LogFollow);
    assert_eq!(page.log_view.content().text(), page.log);
}

fn fake_runner(paths: &Paths) {
    let bin = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join("fake")
        .join("bin");
    fs::create_dir_all(&bin).unwrap();
    let wine = bin.join("wine");
    write_script(
        &wine,
        "#!/bin/sh\nif [ \"$1\" = wineboot ]; then\n  mkdir -p \"$WINEPREFIX/drive_c\"\n  printf 'WINE REGISTRY Version 2\\n' > \"$WINEPREFIX/system.reg\"\nfi\nexit 0\n",
    );
    let wineserver = bin.join("wineserver");
    write_script(&wineserver, "#!/bin/sh\nexit 0\n");
}

#[test]
fn details_and_artwork_fit_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let page = page();
    assert_eq!(page.stage, Stage::Details);
    for (width, height) in [(280.0, 240.0), (900.0, 720.0)] {
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, iced::Size::new(width, height));
        assert!(!probe.scrollables.is_empty());
        let frame = iced::Rectangle::with_size(iced::Size::new(width, height));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
    }
}

#[test]
fn target_programs_fit_narrow_and_small_dialogs() {
    use crate::layout_probe::probe;
    let mut page = page();
    page.stage = Stage::Target;
    let mut report = Report::new();
    report.programs = vec![
        ("Game".to_owned(), "C:\\Games\\Game\\game.exe".to_owned()),
        (
            "A Rather Long Program Name That Needs More Than One Line".to_owned(),
            format!(
                "C:\\Program Files\\{}\\launcher.exe",
                "VeryLongFolderName".repeat(6)
            ),
        ),
        (
            "Editor".to_owned(),
            "C:\\Games\\Game\\editor.exe".to_owned(),
        ),
    ];
    page.report = Some(report);
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, iced::Size::new(width, height));
        assert!(!probe.scrollables.is_empty());
        let frame = iced::Rectangle::with_size(iced::Size::new(width, height));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
    }
}

#[test]
fn back_from_review_preserves_details() {
    let mut page = page();
    page.draft.title = "test-game".to_owned();
    page.draft.path = "/games/test.exe".to_owned();
    page.stage = Stage::Review;
    let _ = page.back();
    assert_eq!(page.stage, Stage::Details);
    assert_eq!(page.draft.path, "/games/test.exe");
}

#[test]
fn details_back_returns_to_add_choices() {
    let mut page = page();
    let _ = page.back();
    assert!(matches!(page.take_action(), Some(Action::Choose)));

    page.draft.title = "draft".to_owned();
    let _ = page.back();
    assert!(page.leave_prompt);
    let _ = page.update(Event::Discard);
    assert!(matches!(page.take_action(), Some(Action::Choose)));
}

#[test]
fn typing_a_title_derives_the_environment_name_and_seeds_art_search() {
    let mut page = page();
    let _ = page.update(Event::Name("The Sims 4".to_owned()));
    assert_eq!(page.draft.environment_name(), "the-sims-4");
    assert_eq!(page.art.search_text(), "The Sims 4");
    assert!(page.draft != Draft::default(), "a title counts as an edit");
}

#[test]
fn edited_environment_name_stops_following_the_title_until_cleared() {
    let mut page = page();
    let _ = page.update(Event::Name("The Sims 4".to_owned()));
    let _ = page.update(Event::EditEnvironment);
    let _ = page.update(Event::EnvironmentName("sims".to_owned()));
    let _ = page.update(Event::Name("The Sims 5".to_owned()));
    assert_eq!(page.draft.environment_name(), "sims");

    let _ = page.update(Event::EnvironmentName(String::new()));
    assert_eq!(page.draft.environment_name(), "the-sims-5");
    let _ = page.update(Event::Name("Hades".to_owned()));
    assert_eq!(page.draft.environment_name(), "hades");

    let _ = page.update(Event::EnvironmentName("bad name".to_owned()));
    assert_eq!(page.validate().unwrap_err(), "spaces not allowed");
}

#[test]
fn a_long_override_is_refused_but_a_long_name_is_cut() {
    let mut page = page();
    // Within the title limit but well past the environment name's.
    let _ = page.update(Event::Name("word ".repeat(20)));
    assert!(page.draft.title.trim().len() > name::TITLE_MAX);
    assert!(page.draft.environment_name().len() <= name::TITLE_MAX);
    assert!(page.validate().is_ok());

    let _ = page.update(Event::EnvironmentName("a".repeat(name::TITLE_MAX + 1)));
    assert_eq!(page.validate().unwrap_err(), "Name is too long.");
    let _ = page.update(Event::EnvironmentName("a".repeat(name::TITLE_MAX)));
    assert!(page.validate().is_ok());
}

#[test]
fn a_title_without_letters_or_digits_needs_an_override() {
    let mut page = page();
    let _ = page.update(Event::Name("\u{30cf}\u{30fc}\u{30c7}\u{30b9}".to_owned()));
    assert_eq!(
        page.validate().unwrap_err(),
        "Name needs letters or digits."
    );
    let _ = page.update(Event::EnvironmentName("hades".to_owned()));
    assert!(page.validate().is_ok());
}

#[test]
fn executable_creation_uses_the_slug_for_the_environment_and_the_title_for_the_target() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let exe = root.path().join("game.exe");
    fs::write(&exe, "").unwrap();
    let draft = Draft {
        title: "The Sims 4".to_owned(),
        source: Source::Executable,
        path: exe.display().to_string(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let runner = review.runner.clone().unwrap();
    let request = create_request(&draft, review.clone(), runner);
    assert_eq!(request.name, "the-sims-4");
    assert_eq!(request.target_name.as_deref(), Some("The Sims 4"));

    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    let environment = Environment::load(&paths, "the-sims-4").unwrap();
    assert_eq!(
        environment.config.targets["the-sims-4"].name.as_deref(),
        Some("The Sims 4")
    );
}

#[test]
fn native_executable_titles_its_target() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let exe = root.path().join("game.sh");
    write_script(&exe, "#!/bin/sh\n");
    let draft = Draft {
        title: "Half-Life 2".to_owned(),
        kind: Kind::Native,
        source: Source::Executable,
        path: exe.display().to_string(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    let environment = Environment::load(&paths, "half-life-2").unwrap();
    assert_eq!(
        environment.config.targets["half-life-2"].name.as_deref(),
        Some("Half-Life 2")
    );
}

fn card_title_of(paths: &Paths, name: &str) -> String {
    crate::library::load_library(paths)
        .unwrap()
        .into_iter()
        .find(|card| card.name == name)
        .unwrap()
        .display_title
}

fn read_config(paths: &Paths, name: &str) -> String {
    fs::read_to_string(paths.environment_config(name)).unwrap()
}

#[test]
fn an_empty_windows_registration_shows_the_typed_name_on_its_card() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let draft = Draft {
        title: "Hollow Knight".to_owned(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert!(read_config(&paths, "hollow-knight").contains("title = \"Hollow Knight\"\n"));
    assert_eq!(card_title_of(&paths, "hollow-knight"), "Hollow Knight");
}

#[test]
fn an_empty_native_registration_shows_the_typed_name_on_its_card() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let draft = Draft {
        title: "Hollow Knight".to_owned(),
        kind: Kind::Native,
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert!(read_config(&paths, "hollow-knight").contains("title = \"Hollow Knight\"\n"));
    assert_eq!(card_title_of(&paths, "hollow-knight"), "Hollow Knight");
}

#[test]
fn executables_without_a_desktop_entry_still_show_the_typed_name() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let exe = root.path().join("game.exe");
    fs::write(&exe, "").unwrap();
    let draft = Draft {
        title: "Hollow Knight".to_owned(),
        source: Source::Executable,
        path: exe.display().to_string(),
        runner: "fake".to_owned(),
        desktop: false,
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let runner = review.runner.clone().unwrap();
    let request = create_request(&draft, review.clone(), runner);
    assert_eq!(request.title.as_deref(), Some("Hollow Knight"));
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert_eq!(card_title_of(&paths, "hollow-knight"), "Hollow Knight");
}

#[test]
fn an_installer_target_added_with_desktop_off_keeps_the_typed_name() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let installer = root.path().join("setup.msi");
    fs::write(&installer, "fixture").unwrap();
    let draft = Draft {
        title: "Hollow Knight".to_owned(),
        source: Source::Installer,
        path: installer.display().to_string(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft.clone(),
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);

    // The Target step's own write, with "Create desktop entry" left off.
    let environment = Environment::load(&paths, "hollow-knight").unwrap();
    let mut batch = Batch::new(environment.config.targets);
    batch
        .add(
            "hollow-knight",
            target_spec("C:\\Game\\game.exe".to_owned(), draft.target_title(), false),
        )
        .unwrap();
    batch.set_default(Some("hollow-knight"));
    assert!(matches!(
        targets::write(&paths, "hollow-knight", &batch).unwrap(),
        Written::Saved
    ));
    assert_eq!(card_title_of(&paths, "hollow-knight"), "Hollow Knight");
}

#[test]
fn an_empty_name_with_an_environment_override_creates_without_a_title() {
    for kind in [Kind::Windows, Kind::Native] {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        fake_runner(&paths);
        let draft = Draft {
            environment: Some("my-env".to_owned()),
            kind,
            runner: "fake".to_owned(),
            ..Draft::default()
        };
        assert_eq!(draft.target_title(), None);
        let review = prepare(&paths, &draft).unwrap();
        let report = perform(
            &paths,
            draft,
            review,
            artwork::Draft::new(),
            Token::new(),
            |_| {},
            |_| {},
        );
        assert!(report.registered, "{kind}: {:?}", report.notes);
        assert!(
            !read_config(&paths, "my-env").contains("title"),
            "{kind}: {}",
            read_config(&paths, "my-env")
        );
        assert_eq!(card_title_of(&paths, "my-env"), "my-env");
    }
}

#[test]
fn a_name_with_control_characters_is_refused_with_the_core_message() {
    let mut page = page();
    let _ = page.update(Event::Name("Hollow\u{7}Knight".to_owned()));
    assert_eq!(
        page.validate().unwrap_err(),
        "Invalid title: can't contain control characters."
    );
}

#[test]
fn a_name_at_the_title_limit_is_written_as_the_title() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    // Two-byte characters, so the limit is counted in characters, not bytes.
    let title = "\u{e9}".repeat(TITLE_MAX_CHARS);
    let mut page = Page::new(paths.clone()).0;
    let _ = page.update(Event::Kind(Kind::Native));
    let _ = page.update(Event::Name(title.clone()));
    // The name can't come from letters like these, so it is typed.
    let _ = page.update(Event::EnvironmentName("long-title".to_owned()));
    assert_eq!(page.validate(), Ok(()));
    let _ = page.update(Event::Next);
    assert!(page.error.is_none());
    assert!(page.reviewing.is_some());

    let draft = Draft {
        title: title.clone(),
        environment: Some("long-title".to_owned()),
        kind: Kind::Native,
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert_eq!(card_title_of(&paths, "long-title"), title);
}

#[test]
fn a_name_over_the_title_limit_is_refused_until_it_is_shortened() {
    let mut page = page();
    let _ = page.update(Event::Name("a".repeat(TITLE_MAX_CHARS + 1)));
    let message = format!("Invalid title: can't be longer than {TITLE_MAX_CHARS} characters.");
    assert_eq!(page.validate().unwrap_err(), message);

    let _ = page.update(Event::Next);
    assert_eq!(page.error.as_deref(), Some(message.as_str()));
    assert_eq!(page.stage, Stage::Details);
    assert!(page.reviewing.is_none());
    // Create has no review to act on, and starting it anyway re-checks the Name.
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
    page.stage = Stage::Review;
    let _ = page.update(Event::Create);
    assert_eq!(page.error.as_deref(), Some(message.as_str()));
    assert!(!page.busy());
    page.stage = Stage::Details;

    // Whitespace around the Name doesn't count toward the limit.
    let _ = page.update(Event::Name(format!(" {} ", "a".repeat(TITLE_MAX_CHARS))));
    assert!(page.error.is_none());
    assert_eq!(page.validate(), Ok(()));
}

#[test]
fn native_creation_keeps_library_warning_in_report() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let library = paths.library_file();
    fs::create_dir_all(library.parent().unwrap()).unwrap();
    fs::write(&library, "[broken\n").unwrap();
    let draft = Draft {
        title: "Broken Library".to_owned(),
        kind: Kind::Native,
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert!(report.notes.iter().any(|note| {
        note.starts_with("Can't update library.toml:")
            && note.contains("broken-library")
            && note.contains("may show an old star or hidden flag once it can be read")
    }));
    assert_eq!(fs::read(&library).unwrap(), b"[broken\n");
}

#[test]
fn picked_installer_program_takes_the_title() {
    let spec = target_spec(
        "C:\\Game\\game.exe".to_owned(),
        Some("Hades".to_owned()),
        true,
    );
    assert_eq!(spec.name.as_deref(), Some("Hades"));
    assert!(spec.desktop);
    assert_eq!(target_spec(String::new(), None, false).name, None);
}

#[test]
fn template_preview_names_only_configured_hook_phases() {
    let draft = page().draft;
    for (before, after, expected) in [
        (true, true, Some("Hooks: before, after")),
        (true, false, Some("Hooks: before")),
        (false, true, Some("Hooks: after")),
        (false, false, None),
    ] {
        let template = Template {
            hooks: Some(wineo_core::config::HooksSetting::On(
                wineo_core::config::Hooks {
                    before: before.then(|| "private command".into()),
                    after: after.then(|| "private command".into()),
                },
            )),
            ..Template::default()
        };
        let effects = template_effects(&template, &draft);
        assert_eq!(
            effects
                .iter()
                .find(|line| line.starts_with("Hooks:"))
                .map(String::as_str),
            expected
        );
        assert!(effects.iter().all(|line| !line.contains("private command")));
    }
}

#[test]
fn gaming_template_shows_effects_and_extra_options_only_for_windows() {
    let mut page = page();
    let _ = page.update(Event::Template("gaming".to_owned()));
    let effects = template_effects(page.template_preview.as_ref().unwrap(), &page.draft);
    assert_eq!(
        effects,
        [
            "Runner: newest installed Proton, or the global default",
            "Graphics: DXVK latest + vkd3d latest (Proton uses its bundled versions)",
            "GameMode: on",
            "Tags: game"
        ]
    );
    assert!(!page.additional_wine_options_visible());
    let _ = page.update(Event::ToggleAdditionalWineOptions);
    assert!(page.additional_wine_options_visible());
    let _ = page.update(Event::Kind(Kind::Native));
    assert!(!page.additional_wine_options_visible());
}

#[test]
fn finished_launch_needs_opt_in_and_playable_target() {
    let mut page = page();
    page.draft.title = "test-game".to_owned();
    page.report = Some(Report {
        registered: true,
        playable: true,
        cleanup_resolved: true,
        ..Report::new()
    });
    page.finish();
    let _ = page.update(Event::ToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { launch: false, .. })
    ));

    page.launch = true;
    page.finish();
    assert!(page.launch, "artwork retry preserves the launch choice");
    if let Some(report) = &mut page.report {
        report.cleanup_resolved = false;
    }
    let _ = page.update(Event::ToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { launch: false, .. })
    ));

    if let Some(report) = &mut page.report {
        report.cleanup_resolved = true;
    }
    let _ = page.update(Event::ToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { launch: true, .. })
    ));
}

#[test]
fn installer_creates_environment_without_registering_it_as_target() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let installer = root.path().join("setup.msi");
    fs::write(&installer, "fixture").unwrap();
    let draft = Draft {
        title: "installed-game".to_owned(),
        source: Source::Installer,
        path: installer.display().to_string(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        Token::new(),
        |_| {},
        |_| {},
    );
    assert!(report.registered, "{:?}", report.notes);
    assert!(report.cleanup_resolved, "{:?}", report.notes);
    assert!(!report.playable);
    assert!(
        Environment::load(&paths, "installed-game")
            .unwrap()
            .config
            .targets
            .is_empty()
    );
}

/// Runs the installer step with a wine whose installer run does `installer`.
fn install_with(installer: &str, cancel_when_running: bool) -> Report {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let wine = GlobalConfig::default()
        .runner_root(&paths)
        .unwrap()
        .join("fake/bin/wine");
    write_script(
        &wine,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n\
                 'wineboot --init') mkdir -p \"$WINEPREFIX/drive_c\"; printf 'WINE REGISTRY Version 2\\n' > \"$WINEPREFIX/system.reg\" ;;\n\
                 'wineboot --end-session') touch \"$WINEPREFIX/stopped\" ;;\n\
                 *setup.exe*) {installer} ;;\n\
                 esac\n"
        ),
    );
    let installer = root.path().join("setup.exe");
    fs::write(&installer, "fixture").unwrap();
    let draft = Draft {
        title: "installed-game".to_owned(),
        source: Source::Installer,
        path: installer.display().to_string(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let token = Token::new();
    let cancel = token.clone();
    perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        token,
        |line| {
            if cancel_when_running && line == "Running installer..." {
                cancel.cancel();
            }
        },
        |_| {},
    )
}

#[test]
fn a_failed_installer_still_offers_what_it_installed() {
    let report = install_with(
        "mkdir -p \"$WINEPREFIX/drive_c/Games/Game\"; : > \"$WINEPREFIX/drive_c/Games/Game/Game.exe\"; exit 3",
        false,
    );
    assert!(report.offer_programs, "{:?}", report.notes);
    assert!(
        report
            .notes
            .contains(&"Installer reported an error (code 3)".to_owned()),
        "{:?}",
        report.notes
    );
    assert!(
        report.programs.iter().any(|(name, _)| name == "Game"),
        "{:?}",
        report.programs
    );
}

#[test]
fn a_canceled_installer_skips_the_programs_and_points_to_settings() {
    let report = install_with(
        "while [ ! -e \"$WINEPREFIX/stopped\" ]; do sleep 0.02; done",
        true,
    );
    assert!(report.registered, "{:?}", report.notes);
    assert!(!report.offer_programs, "{:?}", report.notes);
    assert!(report.programs.is_empty());
    assert!(
        report
            .notes
            .contains(&"Installer canceled. Add a target in Settings.".to_owned()),
        "{:?}",
        report.notes
    );
}

#[test]
fn cancellation_before_creation_leaves_no_environment() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let draft = Draft {
        title: "canceled-game".to_owned(),
        runner: "fake".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let token = Token::new();
    token.cancel();
    let report = perform(
        &paths,
        draft,
        review,
        artwork::Draft::new(),
        token,
        |_| {},
        |_| {},
    );
    assert!(!report.registered);
    assert!(!paths.environment_config("canceled-game").exists());
}

fn offer_page(paths: &Paths) -> Page {
    let mut page = Page::new(paths.clone()).0;
    let _ = page.update(Event::Options(read_options(paths)));
    page
}

fn write_global(paths: &Paths, text: &str) {
    let file = paths.global_config();
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

#[test]
fn the_offer_shows_for_a_game_only_while_unanswered_and_not_portable() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(&home))).unwrap();
    let mut page = offer_page(&paths);
    assert!(!page.offer_shown());
    let _ = page.update(Event::Template("gaming".to_owned()));
    assert!(page.offer_shown());
    let _ = page.update(Event::Kind(Kind::Native));
    assert!(!page.offer_shown());

    write_global(&paths, "external_runners = false\n");
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    assert!(!page.offer_shown());

    let portable = root.path().join("portable");
    let paths = Paths::select(
        |key| (key == "HOME").then(|| OsString::from(&home)),
        Some(&portable),
    )
    .unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    assert!(!page.offer_shown());
}

#[test]
fn answering_the_offer_saves_it_hides_it_and_relists_runners() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    let steam = |page: &Page| {
        page.options
            .as_ref()
            .unwrap()
            .runners
            .iter()
            .any(|choice| choice.to_string() == "GE-Proton11-3 (Steam)")
    };
    assert!(!steam(&page));

    let _ = page.update(Event::Offer(true));
    assert!(page.answering);
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        answer_offer(&paths, true),
    ));
    assert!(!page.answering);
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(true)
    );
    assert!(!page.offer_shown());
    assert!(steam(&page));
    // Answered, it never comes back.
    let _ = page.update(Event::Offer(false));
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(true)
    );
}

#[test]
fn a_late_answer_keeps_the_review_and_rechecks_a_pending_one() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Name("Hades".to_owned()));

    let _ = page.update(Event::Offer(true));
    let sequence = page.offer_sequence;
    // The buttons are off while it saves, so another click does nothing.
    let _ = page.update(Event::Offer(false));
    assert_eq!(page.offer_sequence, sequence);

    // Next is clicked and its review lands before the answer does.
    let _ = page.update(Event::Next);
    let _ = page.update(Event::Reviewed(
        page.generation,
        prepare(&paths, &page.draft).map(Box::new),
    ));
    assert_eq!(page.stage, Stage::Review);
    let _ = page.update(Event::Offered(sequence, answer_offer(&paths, true)));
    assert_eq!(page.stage, Stage::Review);
    assert!(page.review.is_some());
    assert!(!page.answering);

    // A review still being prepared when the answer lands is redone, so
    // it can't read the old setting.
    fs::remove_file(paths.global_config()).unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Name("Hades".to_owned()));
    let _ = page.update(Event::Offer(true));
    let _ = page.update(Event::Next);
    let stale = page.generation;
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        answer_offer(&paths, true),
    ));
    assert_ne!(page.generation, stale);
    assert_eq!(page.reviewing, Some(page.generation));
    let old = prepare(&paths, &page.draft).map(Box::new);
    let _ = page.update(Event::Reviewed(stale, old));
    assert_eq!(page.stage, Stage::Details);
    // A reply from an earlier answer is ignored.
    let _ = page.update(Event::Offered(
        page.offer_sequence.wrapping_sub(1),
        Err("old".to_owned()),
    ));
    assert!(page.error.is_none());
}

#[test]
fn the_review_runner_row_names_another_launchers_build() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    write_global(&paths, "external_runners = true\n");
    let global = GlobalConfig::load(&paths).unwrap();
    let runners = Runners::new(&paths, &global).unwrap();
    let steam = runners.resolve("GE-Proton11-3").unwrap();
    assert_eq!(
        runner_label::resolved(&paths, &steam),
        "GE-Proton11-3 (Steam)"
    );
    fake_runner(&paths);
    let own = runners.resolve("fake").unwrap();
    assert_eq!(runner_label::resolved(&paths, &own), "fake (fake)");

    let draft = Draft {
        title: "steam-game".to_owned(),
        runner: "GE-Proton11-3".to_owned(),
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    assert_eq!(review.runner_row.as_deref(), Some("GE-Proton11-3 (Steam)"));
}

fn fake_proton(paths: &Paths, name: &str) {
    let dir = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join(name);
    fs::create_dir_all(&dir).unwrap();
    write_script(dir.join("proton"), "#!/bin/sh\nexit 0\n");
}

/// A Windows draft that asks for a Proton build that isn't installed, with a
/// template that sets a 32-bit architecture and a DXVK pick, and a compatible
/// Proton prefix already sitting where the environment would go. Returns the
/// check before the download and the one after the build is installed.
fn before_and_after_a_download(runner: &str) -> (Review, Review) {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fs::create_dir_all(paths.templates_dir()).unwrap();
    fs::write(
        paths.templates_dir().join("old.toml"),
        "arch = \"win32\"\ndxvk = \"2.4\"\n",
    )
    .unwrap();
    let draft = Draft {
        title: "old-game".to_owned(),
        template: "old".to_owned(),
        runner: runner.to_owned(),
        arch: Some(Arch::Win32),
        ..Draft::default()
    };
    let prefix = prepare(&paths, &draft).unwrap().prefix.unwrap();
    fs::create_dir_all(prefix.join("pfx")).unwrap();
    fs::write(prefix.join("pfx/system.reg"), "WINE REGISTRY Version 2\n").unwrap();

    let before = prepare(&paths, &draft).unwrap();
    assert!(before.runner.is_none());
    fake_proton(&paths, "GE-Proton11-3");
    let draft = Draft {
        runner: "GE-Proton11-3".to_owned(),
        ..draft
    };
    let after = prepare(&paths, &draft).unwrap();
    assert!(after.runner.as_ref().is_some_and(Runner::is_proton));
    (before, after)
}

#[test]
fn a_missing_runner_shows_the_architecture_the_download_will_use() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3");
    assert_eq!(before.arch, Arch::Win64);
    assert_eq!(before.arch, after.arch);
}

#[test]
fn a_missing_runner_shows_the_prefix_it_will_adopt() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3");
    assert!(before.adopted);
    assert_eq!(before.adopted, after.adopted);
    assert_eq!(before.prefix, after.prefix);
}

#[test]
fn a_missing_runner_shows_the_components_proton_will_omit() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3");
    assert!(before.omitted_components);
    assert_eq!(before.omitted_components, after.omitted_components);
    assert_eq!(before.template, after.template);
}

#[test]
fn a_missing_runner_changes_only_its_own_row_after_the_download() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3");
    assert!(matches!(
        &before.prerequisite,
        Some(Prerequisite::Runner(name)) if name == "GE-Proton11-3"
    ));
    assert!(after.prerequisite.is_none());
    assert!(before.runner_row.is_none() && after.runner_row.is_some());
}

#[test]
fn a_missing_alias_is_filled_in_the_same_way() {
    let (before, _) = before_and_after_a_download("GE-Proton-Latest");
    assert_eq!(before.arch, Arch::Win64);
    assert!(before.adopted);
    assert!(before.omitted_components);
}

/// The app on the New page's working step, and the generation its reply
/// needs.
fn working() -> (crate::App, u64) {
    let mut app = crate::tests::app();
    let mut page = page();
    page.stage = Stage::Working;
    let generation = page.generation;
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    (app, generation)
}

fn done(app: &mut crate::App, generation: u64, programs: &[&str]) -> iced::Task<crate::Message> {
    let mut report = Report::new();
    report.registered = true;
    report.offer_programs = true;
    report.programs = programs
        .iter()
        .map(|exe| ((*exe).to_owned(), format!("C:\\Game\\{exe}")))
        .collect();
    app.update(crate::Message::Flow(
        app.flow_generation,
        crate::flow::Event::Create(Box::new(Event::Done(generation, report))),
    ))
}

#[test]
fn the_target_step_opens_on_its_first_program_and_the_arrows_walk_the_list() {
    let (mut app, generation) = working();
    let task = done(
        &mut app,
        generation,
        &["game.exe", "editor.exe", "server.exe"],
    );
    let ids: Vec<String> = ["game.exe", "editor.exe", "server.exe"]
        .iter()
        .map(|exe| program_id(&format!("C:\\Game\\{exe}")))
        .collect();
    let mut laid = crate::focus_button::landed(&app, task);
    assert_eq!(laid.focused(), [iced::widget::Id::from(ids[0].clone())]);
    crate::focus_button::walk_rows(&mut laid, &ids);
}

#[test]
fn a_target_step_with_no_programs_focuses_no_row() {
    let (mut app, generation) = working();
    let task = done(&mut app, generation, &[]);
    let mut laid = crate::focus_button::landed(&app, task);
    assert!(laid.focused().is_empty());
}

fn reviewed(prerequisite: Option<Prerequisite>) -> Page {
    let mut page = page();
    page.draft.title = "Test game".to_owned();
    page.draft.path = format!("/games/{}/test.exe", "a-long-folder/".repeat(12));
    page.stage = Stage::Review;
    page.review = Some(Review {
        runner: None,
        runner_row: Some("fake".to_owned()),
        template: None,
        arch: Arch::Win64,
        prefix: Some(PathBuf::from("/prefixes/test-game")),
        adopted: false,
        omitted_components: false,
        prerequisite,
    });
    page
}

/// Back sits alone at the top, the content scrolls under it, and the forward
/// action is alone in the pinned strip at the bottom, even in a short dialog.
#[test]
fn back_is_at_the_top_and_the_forward_action_is_pinned_at_the_bottom() {
    use crate::layout_probe::{assert_back_on_top_and_forward_pinned, click_at, probe};
    // What the pinned button publishes; Create is disabled until the runner
    // it needs is downloaded.
    let pages = [
        ("details", page(), Some("Next")),
        ("review", reviewed(None), Some("Create")),
        (
            "review with a missing runner",
            reviewed(Some(Prerequisite::Runner("GE-Proton-Latest".to_owned()))),
            None,
        ),
    ];
    for (label, page, forward) in &pages {
        for (width, height) in [(280.0, 240.0), (900.0, 240.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, iced::Size::new(width, height));
            let case = format!("{label} at {width}x{height}");
            assert_back_on_top_and_forward_pinned(&probe, inset, &case);
        }
        let size = iced::Size::new(900.0, 720.0);
        let view = || page.view_with_key_availability(Palette::DARK, true);
        assert_eq!(click_at(view(), size, &[0, 0, 0]), ["Back"], "{label}");
        let expected: Vec<&str> = forward.iter().copied().collect();
        assert_eq!(click_at(view(), size, &[0, 2, 0]), expected, "{label}");
    }
}

/// The discard prompt shares Back's slot ahead of the scroll body, so showing
/// and dismissing it keeps the body's scroll offset.
#[test]
fn the_discard_prompt_keeps_the_scroll_offset_and_stays_in_view() {
    use crate::layout_probe::{bounds_at, lay_out, probe, relay_out, within};
    let mut page = page();
    page.draft.title = "draft".to_owned();
    let size = iced::Size::new(900.0, 300.0);
    let mut laid = lay_out(page.view_with_key_availability(Palette::DARK, true), size);
    let over_body = iced::mouse::Cursor::Available(iced::Point::new(450.0, 150.0));
    let wheel = iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
        delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
    });
    let _ = laid.event_at(&wheel, over_body);
    let scrolled = laid.offsets()[0];
    assert!(scrolled > 0.0, "the form should scroll in {size:?}");
    for prompt in [true, false] {
        let tree = laid.into_tree();
        page.leave_prompt = prompt;
        laid = relay_out(
            page.view_with_key_availability(Palette::DARK, true),
            tree,
            size,
        );
        assert_eq!(
            laid.offsets(),
            [scrolled],
            "the offset should survive the prompt being {}",
            if prompt { "shown" } else { "dismissed" }
        );
    }

    drop(laid);
    page.leave_prompt = true;
    for (width, height) in [(280.0, 240.0), (900.0, 240.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, iced::Size::new(width, height));
        let case = format!("{width}x{height}");
        let prompt = bounds_at(&probe.node, &[0, 0, 0]);
        let back = bounds_at(&probe.node, &[0, 0, 1]);
        assert!(within(prompt, inset), "{case}: {prompt:?}");
        assert!(within(back, inset), "{case}: {back:?}");
        assert!(
            prompt.y + prompt.height <= back.y + 0.01,
            "{case}: {prompt:?} should be above {back:?}"
        );
        for button in 0..2 {
            let bounds = bounds_at(&probe.node, &[0, 0, 0, 1, button]);
            assert!(within(bounds, inset), "{case}: {bounds:?}");
        }
        let (viewport, _) = probe.scrollables[0];
        assert!(
            viewport.y >= back.y + back.height - 0.01,
            "{case}: {viewport:?} should stay under {back:?}"
        );
    }
}

#[test]
fn a_second_back_dismisses_the_discard_prompt_and_keeps_the_draft() {
    let mut page = page();
    page.draft.title = "draft".to_owned();
    let _ = page.update(Event::Back);
    assert!(page.leave_prompt);
    // Esc sends Back again, which must answer the prompt like Keep editing.
    let _ = page.update(Event::Back);
    assert!(!page.leave_prompt);
    assert_eq!(page.draft.title, "draft");
    assert_eq!(page.stage, Stage::Details);
    assert!(page.take_action().is_none());
}

#[test]
fn other_events_stay_blocked_while_the_discard_prompt_shows() {
    let mut page = page();
    page.draft.title = "draft".to_owned();
    let _ = page.update(Event::Back);
    let kind = page.draft.kind;
    let _ = page.update(Event::Kind(if kind == Kind::Native {
        Kind::Windows
    } else {
        Kind::Native
    }));
    assert!(page.leave_prompt);
    assert_eq!(page.draft.kind, kind);
}
