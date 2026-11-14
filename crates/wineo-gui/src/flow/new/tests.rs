use crate::test_script::write_script;
use std::ffi::OsString;
use std::sync::Arc;

use super::*;
use wineo_core::config::TITLE_MAX_CHARS;

/// A new page, on the choice page it opens on.
fn choosing() -> Page {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let page = Page::new(paths).0;
    assert_eq!(page.stage, Stage::Choose);
    page
}

/// Details for an empty Windows environment, picked on the choice page.
fn page() -> Page {
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
    assert_eq!(page.stage, Stage::Details);
    page
}

/// Goes back to the choice page with Change and picks another row.
fn switch(page: &mut Page, kind: Kind, source: Source) {
    let _ = page.update(Event::Change);
    assert_eq!(page.stage, Stage::Choose);
    let _ = page.update(Event::Start(kind, source));
    assert_eq!(page.stage, Stage::Details);
}

/// A check that found the runner installed and nothing missing.
fn ready_review() -> Review {
    Review {
        runner: None,
        runner_row: Some("fake".to_owned()),
        template: None,
        arch: Arch::Win64,
        prefix: Some(PathBuf::from("/prefixes/test-game")),
        adopted: false,
        omitted_components: false,
        prerequisite: None,
    }
}

/// Lands `review` as the check of the draft as it is now.
fn land(page: &mut Page, review: Review) {
    let _ = page.update(Event::Checked(
        page.check_sequence,
        Box::new(page.draft.clone()),
        Ok(Box::new(review)),
    ));
    assert!(!page.checking);
}

/// The draft stays put after its last edit: the check that wait starts runs.
fn settle(page: &mut Page) {
    let task = page.update(Event::Settled(page.check_sequence));
    drive(page, task, true);
}

/// Whether `task` does nothing at all.
fn is_none(task: Task<Event>) -> bool {
    iced_runtime::task::into_stream(task).is_none()
}

/// Runs an edit's wait with tokio's clock paused, so it passes at once, and
/// says what it published and how long it waited.
fn waited(task: Task<Event>) -> (Vec<Event>, Duration) {
    use iced::futures::StreamExt;
    let mut stream = iced_runtime::task::into_stream(task).expect("an edit starts a wait");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let start = tokio::time::Instant::now();
        let mut events = Vec::new();
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(event) = action {
                events.push(event);
            }
        }
        (events, start.elapsed())
    })
}

/// Each text `page` shows laid out in `size`, with its bounds, in tree order.
fn texts(page: &Page, size: iced::Size) -> Vec<(String, iced::Rectangle)> {
    use iced::advanced::widget::Operation;
    use std::sync::Mutex;
    struct Texts(Arc<Mutex<Vec<(String, iced::Rectangle)>>>);
    impl Operation for Texts {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text(&mut self, _id: Option<&iced::widget::Id>, bounds: iced::Rectangle, text: &str) {
            self.0.lock().unwrap().push((text.to_owned(), bounds));
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    let mut laid =
        crate::layout_probe::lay_out(page.view_with_key_availability(Palette::DARK, true), size);
    laid.run(Box::new(Texts(found.clone())));
    found.lock().unwrap().clone()
}

/// The bounds of each text reading `label`, in tree order.
fn labeled(texts: &[(String, iced::Rectangle)], label: &str) -> Vec<iced::Rectangle> {
    texts
        .iter()
        .filter(|(text, _)| text == label)
        .map(|(_, bounds)| *bounds)
        .collect()
}

/// The last text reading `label`. The pinned strip comes after the body, so
/// a reason shown in both is the strip's.
fn last_labeled(texts: &[(String, iced::Rectangle)], label: &str) -> iced::Rectangle {
    *labeled(texts, label)
        .last()
        .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"))
}

/// What a click on the control labelled `label` publishes, as debug text. A
/// disabled button publishes nothing.
fn click_label(page: &Page, size: iced::Size, label: &str) -> Vec<String> {
    let at = last_labeled(&texts(page, size), label).center();
    let mut laid =
        crate::layout_probe::lay_out(page.view_with_key_availability(Palette::DARK, true), size);
    let cursor = iced::mouse::Cursor::Available(at);
    let left = iced::mouse::Button::Left;
    let mut published = laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
        cursor,
    );
    published.extend(laid.event_at(
        &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
        cursor,
    ));
    published.iter().map(|event| format!("{event:?}")).collect()
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
fn details_and_the_fetch_checkbox_fit_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let page = offer_page(&paths);
    assert_eq!(page.stage, Stage::Details);
    assert!(page.fetch_checked(true), "the checkbox shows, on");
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
fn back_from_details_keeps_the_draft_and_checks_it_again() {
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Executable));
    let _ = page.update(Event::Name("test-game".to_owned()));
    let _ = page.update(Event::Path("/games/test.exe".to_owned()));
    let size = iced::Size::new(900.0, 720.0);
    assert_eq!(click_label(&page, size, "Back"), ["Back"]);
    let _ = page.update(Event::Back);
    assert_eq!(page.stage, Stage::Choose);
    let left = page.check_sequence;
    let check = page.update(Event::Start(Kind::Windows, Source::Executable));
    assert_eq!(page.stage, Stage::Details);
    assert_eq!(page.draft.title, "test-game");
    assert_eq!(page.draft.path, "/games/test.exe");
    // At once, under a new number, without waiting for an edit.
    assert!(!is_none(check), "Details checks the kept draft as it opens");
    assert_ne!(page.check_sequence, left);
    assert!(page.checking);
}

#[test]
fn details_back_returns_to_add_choices() {
    let mut page = page();
    let _ = page.back();
    assert_eq!(page.stage, Stage::Choose);
    assert!(page.take_action().is_none());
    let _ = page.back();
    assert!(matches!(page.take_action(), Some(Action::Choose)));

    let mut page = self::page();
    page.draft.title = "draft".to_owned();
    let _ = page.back();
    assert!(!page.leave_prompt, "Details goes back to the choice page");
    let _ = page.back();
    assert!(page.leave_prompt);
    let _ = page.update(Event::Discard);
    assert!(matches!(page.take_action(), Some(Action::Choose)));
}

#[test]
fn typing_a_title_derives_the_environment_name_and_is_what_the_fetch_searches() {
    let mut page = page();
    let _ = page.update(Event::Name("The Sims 4".to_owned()));
    assert_eq!(page.draft.environment_name(), "the-sims-4");
    assert!(page.draft != Draft::default(), "a title counts as an edit");
    let (searched, _) = fetched_with(&mut page, Ok(Vec::new()));
    assert_eq!(searched, ["the-sims-4 The Sims 4"]);
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

    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft.clone(), review, Token::new(), |_| {}, |_| {});
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
        let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let _ = page.update(Event::Start(Kind::Native, Source::Empty));
    let _ = page.update(Event::Name(title.clone()));
    // The name can't come from letters like these, so it is typed.
    let _ = page.update(Event::EnvironmentName("long-title".to_owned()));
    assert_eq!(page.validate(), Ok(()));
    // The check gets past the fields to the worker.
    let _ = page.update(Event::Settled(page.check_sequence));
    assert_eq!(page.problem(), None);
    assert!(page.checking);

    let draft = Draft {
        title: title.clone(),
        environment: Some("long-title".to_owned()),
        kind: Kind::Native,
        ..Draft::default()
    };
    let review = prepare(&paths, &draft).unwrap();
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
    assert!(report.registered, "{:?}", report.notes);
    assert_eq!(card_title_of(&paths, "long-title"), title);
}

#[test]
fn a_name_over_the_title_limit_is_refused_until_it_is_shortened() {
    let mut page = page();
    let _ = page.update(Event::Name("a".repeat(TITLE_MAX_CHARS + 1)));
    let message = format!("Invalid title: can't be longer than {TITLE_MAX_CHARS} characters.");
    assert_eq!(page.validate().unwrap_err(), message);

    // The Summary says so once the draft settles, without the worker, and
    // Create stays off with the reason beside it.
    let task = page.update(Event::Settled(page.check_sequence));
    assert!(is_none(task));
    let refused = Err(Hold::Problem(Problem::Error(message.clone())));
    assert_eq!(page.checked().map(|_| ()), refused);
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
    // A review in hand doesn't help: Create checks the Name again.
    land(&mut page, ready_review());
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
    assert!(!page.busy());
    assert_eq!(page.checked().map(|_| ()), refused);

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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    switch(&mut page, Kind::Native, Source::Empty);
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
    let _ = page.finish();
    let _ = page.update(Event::ToLibrary);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { launch: false, .. })
    ));

    page.launch = true;
    let _ = page.finish();
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
    let report = perform(&paths, draft, review, Token::new(), |_| {}, |_| {});
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
    let report = perform(&paths, draft, review, token, |_| {}, |_| {});
    assert!(!report.registered);
    assert!(!paths.environment_config("canceled-game").exists());
}

/// Details for an empty Windows environment, with the options loaded.
fn offer_page(paths: &Paths) -> Page {
    let mut page = Page::new(paths.clone()).0;
    let _ = page.update(Event::Options(read_options(paths)));
    let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
    assert_eq!(page.stage, Stage::Details);
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
    switch(&mut page, Kind::Native, Source::Empty);
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

/// The Summary's runner row, if its last check found one.
fn runner_row(page: &Page) -> Option<String> {
    let review = page.summary.as_ref()?.found.as_ref().ok()?;
    review.runner_row.clone()
}

#[test]
fn a_late_answer_to_the_offer_always_rechecks_the_summary() {
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

    // The draft settles and its check lands before the answer does, so it
    // read the old setting and doesn't see Steam's build.
    settle(&mut page);
    assert!(!page.checking);
    assert_ne!(runner_row(&page).as_deref(), Some("GE-Proton11-3 (Steam)"));
    let before = page.check_sequence;
    let task = page.update(Event::Offered(sequence, answer_offer(&paths, true)));
    assert!(!page.answering);
    assert_ne!(page.check_sequence, before, "the answer checks again");
    assert_eq!(page.checked().map(|_| ()), Err(Hold::Checking));
    assert!(
        page.summary.is_some(),
        "the last Summary stays while it runs"
    );
    drive(&mut page, task, true);
    assert_eq!(
        runner_row(&page).as_deref(),
        Some("GE-Proton11-3 (Steam)"),
        "{:?}",
        page.summary
    );

    // A check still running when the answer lands is dropped, so it can't
    // show the old setting.
    fs::remove_file(paths.global_config()).unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Name("Hades".to_owned()));
    let _ = page.update(Event::Offer(true));
    let _ = page.update(Event::Settled(page.check_sequence));
    assert!(page.checking);
    let stale = page.check_sequence;
    let old = prepare(&paths, &page.draft).map(Box::new);
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        answer_offer(&paths, true),
    ));
    assert_ne!(page.check_sequence, stale);
    let _ = page.update(Event::Checked(stale, Box::new(page.draft.clone()), old));
    assert!(page.checking, "the old setting's check didn't land");
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

/// Complete enough for the `GE-Proton-Latest` alias to pick it.
fn fake_proton(paths: &Paths, name: &str) {
    let dir = GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join(name);
    fs::create_dir_all(dir.join("files/bin")).unwrap();
    write_script(dir.join("proton"), "#!/bin/sh\nexit 0\n");
    for exe in ["wine", "wineserver"] {
        write_script(dir.join("files/bin").join(exe), "#!/bin/sh\nexit 0\n");
    }
}

/// What a download must not change in a Review: everything but the runner
/// row and its prerequisite. Naming every field means a new one has to be
/// listed here or the build fails.
fn without_the_runner(review: Review) -> (Option<Template>, Arch, Option<PathBuf>, bool, bool) {
    let Review {
        runner: _,
        runner_row: _,
        prerequisite: _,
        template,
        arch,
        prefix,
        adopted,
        omitted_components,
    } = review;
    (template, arch, prefix, adopted, omitted_components)
}

/// A Windows draft that asks for a Proton build that isn't installed, with a
/// template that sets a 32-bit architecture and the given component picks,
/// and a compatible Proton prefix already sitting where the environment would
/// go (in a folder of its own when `custom_prefix` is set). Returns the check
/// before the download and the one after the build is installed. A named
/// runner is replaced by the installed build's name, as the download does; an
/// alias stays.
fn before_and_after_a_download(
    runner: &str,
    components: &str,
    custom_prefix: bool,
) -> (Review, Review) {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fs::create_dir_all(paths.templates_dir()).unwrap();
    fs::write(
        paths.templates_dir().join("old.toml"),
        format!("arch = \"win32\"\n{components}\n"),
    )
    .unwrap();
    let custom = root.path().join("custom-prefix");
    let draft = Draft {
        title: "old-game".to_owned(),
        template: "old".to_owned(),
        runner: runner.to_owned(),
        arch: Some(Arch::Win32),
        prefix: if custom_prefix {
            custom.display().to_string()
        } else {
            String::new()
        },
        ..Draft::default()
    };
    let prefix = prepare(&paths, &draft).unwrap().prefix.unwrap();
    if custom_prefix {
        assert_eq!(prefix, custom);
    }
    fs::create_dir_all(prefix.join("pfx")).unwrap();
    fs::write(prefix.join("pfx/system.reg"), "WINE REGISTRY Version 2\n").unwrap();

    let before = prepare(&paths, &draft).unwrap();
    assert!(before.runner.is_none());
    fake_proton(&paths, "GE-Proton11-3");
    let draft = Draft {
        runner: if wineo_core::runner_alias::is_alias(runner) {
            runner.to_owned()
        } else {
            "GE-Proton11-3".to_owned()
        },
        ..draft
    };
    let after = prepare(&paths, &draft).unwrap();
    assert!(after.runner.as_ref().is_some_and(Runner::is_proton));
    (before, after)
}

const DXVK: &str = "dxvk = \"2.4\"";
const VKD3D: &str = "vkd3d = \"2.12\"";

#[test]
fn a_missing_runner_shows_the_architecture_the_download_will_use() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3", DXVK, false);
    assert_eq!(before.arch, Arch::Win64);
    assert_eq!(before.arch, after.arch);
}

#[test]
fn a_missing_runner_shows_the_prefix_it_will_adopt() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3", DXVK, false);
    assert!(before.adopted);
    assert_eq!(before.adopted, after.adopted);
    assert_eq!(before.prefix, after.prefix);
}

#[test]
fn a_missing_runner_shows_the_prefix_it_will_adopt_in_a_chosen_folder() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3", DXVK, true);
    assert!(before.adopted);
    assert!(
        before
            .prefix
            .as_ref()
            .is_some_and(|p| p.ends_with("custom-prefix"))
    );
    assert_eq!(without_the_runner(before), without_the_runner(after));
}

#[test]
fn a_missing_runner_shows_the_components_proton_will_omit() {
    for components in [DXVK, VKD3D] {
        let (before, after) = before_and_after_a_download("GE-Proton11-3", components, false);
        assert!(before.omitted_components, "{components}");
        assert_eq!(before.omitted_components, after.omitted_components);
    }
}

#[test]
fn a_template_without_component_picks_omits_nothing_before_or_after() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3", "", false);
    assert!(!before.omitted_components);
    assert!(!after.omitted_components);
}

#[test]
fn a_missing_runner_changes_only_its_own_row_after_the_download() {
    let (before, after) = before_and_after_a_download("GE-Proton11-3", DXVK, false);
    assert!(matches!(
        &before.prerequisite,
        Some(Prerequisite::Runner(name)) if name == "GE-Proton11-3"
    ));
    assert!(after.prerequisite.is_none());
    assert!(before.runner_row.is_none() && after.runner_row.is_some());
    assert_eq!(without_the_runner(before), without_the_runner(after));
}

#[test]
fn a_missing_alias_is_filled_in_the_same_way_and_stays_after_the_download() {
    let (before, after) = before_and_after_a_download("GE-Proton-Latest", DXVK, false);
    assert!(matches!(
        &before.prerequisite,
        Some(Prerequisite::Runner(name)) if name == "GE-Proton-Latest"
    ));
    assert!(after.prerequisite.is_none());
    assert_eq!(before.arch, Arch::Win64);
    assert!(before.adopted);
    assert!(before.omitted_components);
    assert_eq!(without_the_runner(before), without_the_runner(after));
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

/// Details for a game with a long path, its check landed with `review`.
fn summarized(review: Review) -> Page {
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Executable));
    let _ = page.update(Event::Name("Test game".to_owned()));
    let _ = page.update(Event::Path(format!(
        "/games/{}/test.exe",
        "a-long-folder/".repeat(12)
    )));
    land(&mut page, review);
    page
}

fn missing_runner() -> Review {
    Review {
        runner_row: None,
        prerequisite: Some(Prerequisite::Runner("GE-Proton-Latest".to_owned())),
        ..ready_review()
    }
}

fn adopting() -> Review {
    Review {
        adopted: true,
        omitted_components: true,
        ..ready_review()
    }
}

/// Back sits alone at the top, the content scrolls under it, and Create is
/// at the left of the pinned strip at the bottom, with why it's off or what
/// it will do beside it, even in a short dialog. Each is found by its label.
#[test]
fn back_is_at_the_top_and_create_is_pinned_at_the_bottom_with_its_reason() {
    use crate::layout_probe::{probe, within};
    let mut checking = summarized(ready_review());
    let _ = checking.update(Event::Name("Test game 2".to_owned()));
    let pages = [
        ("untouched", page(), vec!["Enter a name"], None),
        ("checking", checking, vec!["Checking..."], None),
        (
            "missing runner",
            summarized(missing_runner()),
            vec!["Runner GE-Proton-Latest isn't installed"],
            None,
        ),
        ("ready", summarized(ready_review()), vec![], Some("Create")),
        (
            "ready with notes",
            summarized(adopting()),
            vec![
                "This will register the existing prefix in place. \
                 Proton's own DXVK and vkd3d-proton replace the template's.",
            ],
            Some("Create"),
        ),
    ];
    let mut occupied = summarized(ready_review());
    let error = format!(
        "{}/prefixes/test-game is occupied and is not a compatible prefix",
        "/a-very-long-folder-name".repeat(6)
    );
    occupied.summary.as_mut().unwrap().found = Err(Problem::Error(error.clone()));
    let mut pages = Vec::from(pages);
    // The strip cuts it; the Summary has all of it.
    let pinned = cut(error.clone());
    assert!(pinned.ends_with("...") && pinned.chars().count() == PINNED_REASON_MAX + 3);
    pages.push(("a long error", occupied, vec![pinned.as_str()], None));
    for (label, page, beside, publishes) in &pages {
        for (width, height) in [
            (280.0, 240.0),
            (400.0, 300.0),
            (900.0, 240.0),
            (900.0, 720.0),
        ] {
            let size = iced::Size::new(width, height);
            let case = format!("{label} at {width}x{height}");
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, size);
            let spilled = probe.outside(iced::Rectangle::with_size(size));
            assert!(spilled.is_empty(), "{case}: {spilled:?}");
            assert_eq!(probe.scrollables.len(), 1, "{case}");
            let (viewport, _) = probe.scrollables[0];
            assert!(viewport.height > 0.0, "{case}: {viewport:?}");
            let texts = texts(page, size);
            let back = last_labeled(&texts, "Back");
            let create = last_labeled(&texts, "Create");
            assert!(within(back, inset), "{case}: {back:?}");
            assert!(back.y < inset.y + 32.0, "{case}: Back at the top, {back:?}");
            assert!(back.y + back.height <= viewport.y, "{case}: {back:?}");
            assert!(within(create, inset), "{case}: {create:?}");
            assert!(
                create.y >= viewport.y + viewport.height,
                "{case}: {create:?}"
            );
            // The strip's padding and the button's.
            assert!(
                create.x - inset.x < 30.0,
                "{case}: Create at the left, {create:?}"
            );
            for text in beside {
                let note = last_labeled(&texts, text);
                assert!(within(note, inset), "{case}: {text}: {note:?}");
                assert!(
                    note.y >= viewport.y + viewport.height,
                    "{case}: {text} is pinned, {note:?}"
                );
                assert!(
                    note.x >= create.x + create.width || note.y >= create.y + create.height,
                    "{case}: {text} beside or under Create, {note:?}"
                );
            }
        }
        let size = iced::Size::new(900.0, 720.0);
        assert_eq!(click_label(page, size, "Back"), ["Back"], "{label}");
        let expected: Vec<&str> = publishes.iter().copied().collect();
        assert_eq!(click_label(page, size, "Create"), expected, "{label}");
    }
}

/// The discard prompt shares Back's slot ahead of the scroll body, so showing
/// and dismissing it keeps the body's scroll offset. Today it opens on the
/// choice page, but the slot covers Details too.
#[test]
fn the_discard_prompt_keeps_the_scroll_offset_and_stays_in_view() {
    use crate::layout_probe::{lay_out, probe, relay_out, within};
    let mut page = summarized(adopting());
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
    for (width, height) in [
        (280.0, 240.0),
        (400.0, 300.0),
        (900.0, 240.0),
        (900.0, 720.0),
    ] {
        let size = iced::Size::new(width, height);
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, size);
        let case = format!("{width}x{height}");
        let texts = texts(&page, size);
        let prompt = last_labeled(&texts, "Discard this new environment draft?");
        let back = last_labeled(&texts, "Back");
        assert!(within(prompt, inset), "{case}: {prompt:?}");
        assert!(within(back, inset), "{case}: {back:?}");
        assert!(
            prompt.y + prompt.height <= back.y + 0.01,
            "{case}: {prompt:?} should be above {back:?}"
        );
        for button in ["Keep editing", "Discard"] {
            let bounds = last_labeled(&texts, button);
            assert!(within(bounds, inset), "{case}: {button} {bounds:?}");
            assert!(
                bounds.y + bounds.height <= back.y + 0.01,
                "{case}: {button}"
            );
        }
        let (viewport, _) = probe.scrollables[0];
        assert!(
            viewport.y >= back.y + back.height - 0.01,
            "{case}: {viewport:?} should stay under {back:?}"
        );
    }
    let size = iced::Size::new(900.0, 720.0);
    assert_eq!(click_label(&page, size, "Keep editing"), ["KeepEditing"]);
    assert_eq!(click_label(&page, size, "Discard"), ["Discard"]);
}

#[test]
fn a_second_back_dismisses_the_discard_prompt_and_keeps_the_draft() {
    let mut page = page();
    page.draft.title = "draft".to_owned();
    let _ = page.update(Event::Back);
    let _ = page.update(Event::Back);
    assert!(page.leave_prompt);
    // Esc sends Back again, which must answer the prompt like Keep editing.
    let _ = page.update(Event::Back);
    assert!(!page.leave_prompt);
    assert_eq!(page.draft.title, "draft");
    assert_eq!(page.stage, Stage::Choose);
    assert!(page.take_action().is_none());
}

#[test]
fn other_events_stay_blocked_while_the_discard_prompt_shows() {
    let mut page = page();
    page.draft.title = "draft".to_owned();
    let _ = page.update(Event::Back);
    let _ = page.update(Event::Back);
    let kind = page.draft.kind;
    let _ = page.update(Event::Start(
        if kind == Kind::Native {
            Kind::Windows
        } else {
            Kind::Native
        },
        Source::Empty,
    ));
    assert!(page.leave_prompt);
    assert_eq!(page.draft.kind, kind);
    assert_eq!(page.stage, Stage::Choose);
}

/// What a fake fetcher saw on a call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    name: String,
    title: String,
    /// What `busy_with` said about the environment's lock.
    lock: String,
}

type Calls = Arc<std::sync::Mutex<Vec<Call>>>;

/// A fetcher that records each call and answers `answer`, or an error once
/// its token is canceled.
fn fake_fetcher(answer: Result<Vec<String>, String>) -> (Fetcher, Calls) {
    let calls = Calls::default();
    let seen = calls.clone();
    let fetcher: Fetcher = Arc::new(move |paths, name, title, token| {
        let lock = format!(
            "{:?}",
            wineo_core::operation::Locks::new(paths).busy_with(name)
        );
        seen.lock().unwrap().push(Call {
            name: name.to_owned(),
            title: title.to_owned(),
            lock,
        });
        if token.is_canceled() {
            return Err("canceled".to_owned());
        }
        answer.clone()
    });
    (fetcher, calls)
}

/// Runs `task` to its end and returns the events it produced, giving up
/// after a few seconds so a stuck worker fails the test instead of hanging.
fn outputs(task: Task<Event>) -> Vec<Event> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut events = Vec::new();
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_secs(20), stream.next()).await
        {
            if let iced_runtime::Action::Output(event) = action {
                events.push(event);
            }
        }
        events
    })
}

/// Feeds `task`'s events back into `page`, and theirs, until nothing is left.
fn drive(page: &mut Page, task: Task<Event>, key_available: bool) {
    let mut pending = vec![task];
    for _ in 0..64 {
        let Some(task) = pending.pop() else {
            return;
        };
        for event in outputs(task) {
            pending.push(page.update_with_key_availability(event, key_available));
        }
    }
    panic!("the page kept starting work");
}

/// Starts the fetch on `page` with a fetcher answering `answer`, runs it,
/// and says what it was asked for and how the page ended.
fn fetched_with(page: &mut Page, answer: Result<Vec<String>, String>) -> (Vec<String>, Stage) {
    let (fetcher, calls) = fake_fetcher(answer);
    page.fetcher = fetcher;
    let task = page.start_fetch();
    drive(page, task, true);
    let calls = calls.lock().unwrap();
    (
        calls
            .iter()
            .map(|call| format!("{} {}", call.name, call.title))
            .collect(),
        page.stage,
    )
}

/// A page whose environment registered, with the checkbox on at Create.
fn registered(fetch: bool) -> (Page, Calls) {
    let mut page = page();
    page.draft.title = "Hollow Knight".to_owned();
    page.fetch_art = fetch;
    page.report = Some(Report {
        registered: true,
        ..Report::new()
    });
    let (fetcher, calls) = fake_fetcher(Ok(vec!["No logo artwork found".to_owned()]));
    page.fetcher = fetcher;
    (page, calls)
}

fn call_count(calls: &Calls) -> usize {
    calls.lock().unwrap().len()
}

#[test]
fn the_fetch_runs_after_creation_outside_the_create_lock() {
    for kind in [Kind::Windows, Kind::Native] {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        fake_runner(&paths);
        let mut page = offer_page(&paths);
        let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
        page.fetcher = fetcher;
        switch(&mut page, kind, Source::Empty);
        let _ = page.update(Event::Name("Hollow Knight".to_owned()));
        let _ = page.update(Event::Runner("fake".to_owned()));
        settle(&mut page);
        assert!(page.checked().is_ok(), "{kind}: {:?}", page.summary);
        let task = page.update_with_key_availability(Event::Create, true);
        drive(&mut page, task, true);
        assert_eq!(page.stage, Stage::Finished, "{kind}");
        let report = page.report.as_ref().unwrap();
        assert!(report.registered, "{kind}: {:?}", report.notes);
        assert_eq!(report.art_error, None, "{kind}");
        assert_eq!(
            *calls.lock().unwrap(),
            [Call {
                name: "hollow-knight".to_owned(),
                title: "Hollow Knight".to_owned(),
                lock: "Ok(None)".to_owned(),
            }],
            "{kind}"
        );
    }
}

#[test]
fn skip_back_and_esc_cancel_the_fetch_and_land_on_finished() {
    for how in ["Skip", "Back"] {
        let (mut page, calls) = registered(true);
        let task = page.finish();
        assert_eq!(page.stage, Stage::Working, "{how}");
        assert!(page.fetching, "{how}");
        assert!(!page.busy(), "{how}: fetching doesn't hold the window");
        let token = page.fetch_token.clone().unwrap();
        let _ = match how {
            "Skip" => page.update(Event::SkipFetch),
            // Esc arrives as Back.
            _ => page.back(),
        };
        assert!(token.is_canceled(), "{how}");
        assert_eq!(page.status, "Skipping artwork...", "{how}");
        assert_eq!(page.stage, Stage::Working, "{how}: the worker lands it");
        drive(&mut page, task, true);
        assert_eq!(call_count(&calls), 1, "{how}");
        assert_eq!(page.stage, Stage::Finished, "{how}");
        let report = page.report.as_ref().unwrap();
        assert_eq!(report.art_error, None, "{how}");
        assert!(
            report.notes.contains(&"Artwork skipped".to_owned()),
            "{how}: {:?}",
            report.notes
        );
    }
}

#[test]
fn the_desktop_entry_is_reapplied_after_a_fetch_but_not_after_a_skip() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let exe = root.path().join("game.sh");
    write_script(&exe, "#!/bin/sh\n");
    Environment::create_native_titled(
        &paths,
        &GlobalConfig::default(),
        "game",
        Some(&exe.display().to_string()),
        Some("Game"),
        true,
        |_| {},
    )
    .unwrap();
    let entries = || -> Vec<PathBuf> {
        fs::read_dir(paths.applications_dir())
            .map(|dir| dir.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    };
    // Creation alone leaves the entry to whoever reconciles next.
    assert!(entries().is_empty());
    let (fetcher, _) = fake_fetcher(Ok(Vec::new()));

    let token = Token::new();
    token.cancel();
    assert_eq!(
        fetch_art(&paths, "game", "Game", &token, &fetcher),
        Err("canceled".to_owned())
    );
    assert!(entries().is_empty(), "a skip writes nothing more");

    assert_eq!(
        fetch_art(&paths, "game", "Game", &Token::new(), &fetcher),
        Ok(Vec::new())
    );
    assert_eq!(entries().len(), 1);
}

#[test]
fn retry_artwork_fetches_again_through_the_same_skip() {
    let (mut page, calls) = registered(true);
    let (fetcher, _) = fake_fetcher(Err("offline".to_owned()));
    page.fetcher = fetcher;
    let task = page.finish();
    drive(&mut page, task, true);
    assert_eq!(page.stage, Stage::Finished);
    let error = page.report.as_ref().unwrap().art_error.clone().unwrap();
    assert!(error.starts_with("offline"), "{error}");

    // Retry runs the fetch again even though it already ran once.
    let (fetcher, retried) = fake_fetcher(Ok(vec!["No logo artwork found".to_owned()]));
    page.fetcher = fetcher;
    let task = page.update(Event::RetryArt);
    assert!(page.fetching);
    assert_eq!(page.stage, Stage::Working);
    let _ = page.update(Event::SkipFetch);
    assert!(page.fetch_token.as_ref().unwrap().is_canceled());
    drive(&mut page, task, true);
    assert_eq!(call_count(&retried), 1);
    assert_eq!(call_count(&calls), 0);
    let report = page.report.as_ref().unwrap();
    assert_eq!(report.art_error, None);
    assert!(report.notes.contains(&"Artwork skipped".to_owned()));

    // A retry that gets through clears the error and keeps the notes.
    page.report.as_mut().unwrap().art_error = Some("offline".to_owned());
    let task = page.update(Event::RetryArt);
    drive(&mut page, task, true);
    let report = page.report.as_ref().unwrap();
    assert_eq!(report.art_error, None);
    assert!(report.notes.contains(&"No logo artwork found".to_owned()));
    assert_eq!(call_count(&retried), 2);
}

/// Presses Create on Details, checked, with the key `key_available`, lands
/// a registered creation, and runs whatever finishing starts.
fn create_and_register(page: &mut Page, key_available: bool) {
    page.draft.title = "Hollow Knight".to_owned();
    land(page, ready_review());
    // The creation itself never runs; its result is landed below.
    let _ = page.update_with_key_availability(Event::Create, key_available);
    assert_eq!(page.stage, Stage::Working);
    let report = Report {
        registered: true,
        ..Report::new()
    };
    let task =
        page.update_with_key_availability(Event::Done(page.generation, report), key_available);
    drive(page, task, key_available);
    assert_eq!(page.stage, Stage::Finished);
}

#[test]
fn the_checkbox_is_on_by_default_and_toggling_it_is_not_an_edit() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(true));
    let _ = page.update_with_key_availability(Event::FetchArt(false), true);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(false));
    let _ = page.update_with_key_availability(Event::FetchArt(true), true);
    let _ = page.back();
    let _ = page.back();
    assert!(!page.leave_prompt);
    assert!(matches!(page.take_action(), Some(Action::Choose)));

    // The same page, opened again, creates with the checkbox as it was left.
    let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
    let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
    page.fetcher = fetcher;
    create_and_register(&mut page, true);
    assert_eq!(call_count(&calls), 1);

    let mut page = offer_page(&paths);
    let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
    page.fetcher = fetcher;
    let _ = page.update_with_key_availability(Event::FetchArt(false), true);
    create_and_register(&mut page, true);
    assert_eq!(call_count(&calls), 0, "turned off");
}

#[test]
fn no_key_or_no_kinds_shows_a_note_and_never_fetches() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    assert_eq!(page.art_offer(false), ArtOffer::NoKey);
    let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
    page.fetcher = fetcher;
    create_and_register(&mut page, false);
    assert_eq!(call_count(&calls), 0, "no key");

    write_global(&paths, "[art]\nfetch = []\n");
    let mut page = offer_page(&paths);
    assert_eq!(page.art_offer(true), ArtOffer::Off);
    assert_eq!(page.art_offer(false), ArtOffer::Off);
    let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
    page.fetcher = fetcher;
    create_and_register(&mut page, true);
    assert_eq!(call_count(&calls), 0, "no kinds");
}

#[test]
fn a_key_that_vanishes_before_create_turns_the_fetch_off() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update_with_key_availability(Event::FetchArt(true), true);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(true));
    let (fetcher, calls) = fake_fetcher(Ok(Vec::new()));
    page.fetcher = fetcher;
    create_and_register(&mut page, false);
    assert_eq!(call_count(&calls), 0);
    assert!(!page.fetch_art);
}

#[test]
fn the_fetch_waits_for_target_and_runs_once_it_closes() {
    for leave in [Event::SkipTarget, Event::Back] {
        let (mut page, calls) = registered(true);
        page.report.as_mut().unwrap().offer_programs = true;
        page.stage = Stage::Working;
        let report = page.report.clone().unwrap();
        let _ = page.update(Event::Done(page.generation, report));
        assert_eq!(page.stage, Stage::Target);
        assert_eq!(call_count(&calls), 0);
        let task = page.update(leave.clone());
        assert!(page.fetching, "{leave:?}");
        drive(&mut page, task, true);
        assert_eq!(call_count(&calls), 1, "{leave:?}");
        assert_eq!(page.stage, Stage::Finished);
    }

    // Confirm's target lands, then the fetch runs.
    let (mut page, calls) = registered(true);
    page.stage = Stage::Working;
    let task = page.update(Event::TargetAdded(page.generation, Ok(Vec::new())));
    assert!(page.fetching);
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 1);
    assert_eq!(page.stage, Stage::Finished);
}

#[test]
fn a_canceled_installer_fetches_only_if_it_registered() {
    for registered_env in [true, false] {
        let (mut page, calls) = registered(true);
        page.stage = Stage::Working;
        let mut report = page.report.clone().unwrap();
        report.registered = registered_env;
        report
            .notes
            .push("Installer canceled. Add a target in Settings.".to_owned());
        let task = page.update(Event::Done(page.generation, report));
        drive(&mut page, task, true);
        assert_eq!(page.stage, Stage::Finished);
        assert_eq!(call_count(&calls), usize::from(registered_env));
    }
}

#[test]
fn a_skipped_fetch_does_not_run_again_after_clean_up_returns_to_target() {
    let (mut page, calls) = registered(true);
    page.draft.source = Source::Installer;
    page.report.as_mut().unwrap().cleanup_resolved = false;
    page.stage = Stage::Working;
    let report = page.report.clone().unwrap();
    let task = page.update(Event::Done(page.generation, report));
    assert!(page.fetching);
    let _ = page.update(Event::SkipFetch);
    drive(&mut page, task, true);
    assert_eq!(page.stage, Stage::Finished);
    assert_eq!(call_count(&calls), 1);

    // Clean up installer runs nothing real here; its answer lands as if it did.
    let _ = page.update(Event::CleanUp);
    assert_eq!(page.stage, Stage::Working);
    let _ = page.update(Event::Cleaned(page.generation, Ok(true)));
    assert_eq!(page.stage, Stage::Target);
    let task = page.update(Event::SkipTarget);
    assert!(!page.fetching);
    assert_eq!(page.stage, Stage::Finished);
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 1);
}

#[test]
fn choose_artwork_leaves_for_the_art_page_without_launching() {
    let (mut page, _) = registered(false);
    page.report.as_mut().unwrap().playable = true;
    page.launch = true;
    let _ = page.finish();
    let _ = page.update(Event::ChooseArt);
    assert!(matches!(
        page.take_action(),
        Some(Action::Art { name }) if name == "hollow-knight"
    ));

    // Nothing to choose art for when creation didn't register.
    page.report.as_mut().unwrap().registered = false;
    let _ = page.update(Event::ChooseArt);
    assert!(page.take_action().is_none());
}

#[test]
fn finished_buttons_wrap_in_small_dialogs() {
    use crate::layout_probe::{bounds_at, probe};
    let (mut page, _) = registered(false);
    page.report.as_mut().unwrap().playable = true;
    page.report.as_mut().unwrap().art_error = Some("offline".to_owned());
    let _ = page.finish();
    // The body's children: the title, the library line, the artwork error,
    // Retry, Skip, the launch checkbox, then Open settings, Choose
    // artwork..., and Back to library in a row.
    let body = [0, 0, 0, 0];
    let path = |child: &[usize]| [&body[..], child].concat();
    for (width, height) in [(280.0, 240.0), (400.0, 300.0)] {
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, iced::Size::new(width, height));
        let frame = iced::Rectangle::with_size(iced::Size::new(width, height));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        // Squeezed onto one line, the labels would wrap inside taller buttons.
        let one_line = bounds_at(&probe.node, &path(&[3])).height;
        let buttons: Vec<_> = (0..3)
            .map(|button| bounds_at(&probe.node, &path(&[6, button])))
            .collect();
        for bounds in &buttons {
            assert!(
                (bounds.height - one_line).abs() < 0.01,
                "{width}x{height}: {buttons:?}"
            );
        }
        assert!(
            buttons[2].y > buttons[0].y,
            "{width}x{height}: the row wraps, {buttons:?}"
        );
    }
}

#[test]
fn closing_while_fetching_asks_nothing_and_cancels_the_fetch() {
    let mut app = crate::tests::app();
    let (mut page, _) = registered(true);
    // The fetch's task is dropped unrun; only its token matters here.
    let _ = page.finish();
    assert!(page.fetching);
    let token = page.fetch_token.clone().unwrap();
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    assert!(!app.management_busy(), "fetching blocks nothing");
    assert!(!app.foreground_busy());
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(!app.close_prompt);
    assert!(app.exiting);
    assert!(token.is_canceled());
}

/// The app with a SteamGridDB key, on the New page that `page` is.
fn app_with_key(page: Page) -> crate::App {
    let mut app = crate::tests::app();
    let _ = app.update(crate::Message::SteamGridDbKeyPresenceLoaded(1, true));
    assert!(app.steamgriddb_available);
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    app
}

fn new_page(app: &crate::App) -> &Page {
    match &app.flow {
        Some(crate::flow::Page::New(page)) => page,
        _ => panic!("not on the New page"),
    }
}

#[test]
fn ctrl_enter_on_details_keeps_the_fetch_the_checkbox_shows() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    page.draft.title = "Hollow Knight".to_owned();
    land(&mut page, ready_review());
    let mut app = app_with_key(page);
    assert!(app.flow.as_ref().unwrap().fetch_checked(true));
    // The creation's task comes back unrun.
    let _ = app.update(crate::Message::Key(
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        iced::keyboard::Modifiers::CTRL,
        iced::event::Status::Ignored,
    ));
    let page = new_page(&app);
    assert_eq!(page.stage, Stage::Working);
    assert!(page.fetch_art, "Create kept the checkbox as it showed");
}

/// Whether every way the window can close cancels a fetch that isn't busy.
#[test]
fn a_close_that_waits_still_cancels_the_fetch() {
    // Waiting on a library.toml write, with nothing to ask about.
    let (mut page, _) = registered(true);
    let _ = page.finish();
    let token = page.fetch_token.clone().unwrap();
    let mut app = app_with_key(page);
    // The flow covers the library, so the write started before it opened.
    let flow = app.flow.take();
    let _ = app.update(crate::Message::Sort(
        crate::sort_control::Event::FlipDirection,
    ));
    app.flow = flow;
    assert!(app.library_writes_pending());
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.closing && app.close_quietly && !app.exiting);
    assert!(token.is_canceled());

    // Waiting on foreground work the close prompt asked to stop.
    let (mut page, _) = registered(true);
    let _ = page.finish();
    let token = page.fetch_token.clone().unwrap();
    let mut app = app_with_key(page);
    app.operation = Some(crate::Operation {
        name: "other".into(),
        kind: crate::OperationKind::Start,
        token: Some(Token::new()),
        cancelled: false,
        status: "Starting...".into(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.close_prompt);
    assert!(!token.is_canceled(), "the prompt can still be taken back");
    let _ = app.update(crate::Message::CancelAndClose);
    assert!(app.closing && !app.exiting);
    assert!(token.is_canceled());
}

const ROWS: [(Kind, Source); 5] = [
    (Kind::Windows, Source::Installer),
    (Kind::Windows, Source::Executable),
    (Kind::Windows, Source::Empty),
    (Kind::Native, Source::Executable),
    (Kind::Native, Source::Empty),
];

/// Each focusable's id and bounds on a laid out page, in order.
fn focusable_bounds<M>(
    laid: &mut crate::layout_probe::Laid<'_, M>,
) -> Vec<(String, iced::Rectangle)> {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::Focusable;
    use std::sync::Mutex;
    struct All(Arc<Mutex<Vec<(String, iced::Rectangle)>>>);
    impl Operation for All {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(
            &mut self,
            id: Option<&iced::widget::Id>,
            bounds: iced::Rectangle,
            _state: &mut dyn Focusable,
        ) {
            if let Some(id) = id {
                self.0.lock().unwrap().push((format!("{id:?}"), bounds));
            }
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    laid.run(Box::new(All(found.clone())));
    found.lock().unwrap().clone()
}

/// The choice page's rows' bounds, in order.
fn start_rows(laid: &mut crate::layout_probe::Laid<'_, Event>) -> Vec<iced::Rectangle> {
    focusable_bounds(laid)
        .into_iter()
        .filter(|(id, _)| STARTS.iter().any(|start| id.contains(start.id)))
        .map(|(_, bounds)| bounds)
        .collect()
}

#[test]
fn the_choice_page_lists_windows_then_native_rows_that_open_details() {
    use crate::layout_probe::lay_out;
    let page = choosing();
    let rows = page.rows().unwrap();
    assert_eq!(rows.list, START_LIST);
    assert_eq!(rows.ids.len(), ROWS.len());
    assert_eq!(rows.focus, None, "the first row has focus when it opens");

    let size = iced::Size::new(900.0, 720.0);
    let mut laid = lay_out(page.view_with_key_availability(Palette::DARK, true), size);
    let bounds = start_rows(&mut laid);
    assert_eq!(bounds.len(), ROWS.len());
    for (row, (kind, source)) in bounds.iter().zip(ROWS) {
        let at = iced::mouse::Cursor::Available(row.center());
        let left = iced::mouse::Button::Left;
        let mut published = laid.event_at(
            &iced::Event::Mouse(iced::mouse::Event::ButtonPressed(left)),
            at,
        );
        published.extend(laid.event_at(
            &iced::Event::Mouse(iced::mouse::Event::ButtonReleased(left)),
            at,
        ));
        let published: Vec<_> = published.iter().map(|e| format!("{e:?}")).collect();
        assert_eq!(published, [format!("Start({kind:?}, {source:?})")]);
    }
    // Inside a section only a separator parts the rows; the Native Linux
    // heading and its own surface part the sections.
    let gap = |above: usize| bounds[above + 1].y - (bounds[above].y + bounds[above].height);
    assert!(gap(2) > gap(1) + 10.0, "{bounds:?}");
    assert!((gap(0) - gap(1)).abs() < 0.01 && (gap(3) - gap(0)).abs() < 0.01);

    for (index, (kind, source)) in ROWS.into_iter().enumerate() {
        let digit = (index + 1).to_string();
        assert!(
            matches!(Page::numbered(&digit), Some(Event::Start(k, s)) if (k, s) == (kind, source)),
            "{digit}"
        );
        let mut page = choosing();
        let _ = page.update(Event::Start(kind, source));
        assert_eq!(page.details_for(), Some((kind, source)));
        assert!(page.rows().is_none(), "Details has no rows");
    }
    for digit in ["0", "6", "a"] {
        assert!(Page::numbered(digit).is_none(), "{digit}");
    }
    // A Native installer is no row.
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Native, Source::Installer));
    assert_eq!(page.stage, Stage::Choose);
}

#[test]
fn coming_back_to_the_choice_page_lands_on_the_row_details_came_from() {
    for (kind, source) in ROWS {
        for back in [Event::Back, Event::Change] {
            let mut page = choosing();
            let _ = page.update(Event::Start(kind, source));
            let _ = page.update(back.clone());
            assert_eq!(page.stage, Stage::Choose);
            let id = start_of(kind, source).unwrap().id;
            assert_eq!(page.rows().unwrap().focus.as_deref(), Some(id), "{back:?}");
        }
    }
}

#[test]
fn back_from_untouched_details_or_choose_asks_nothing() {
    for (kind, source) in ROWS {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let mut page = Page::new(paths.clone()).0;
        let _ = page.update(Event::Options(read_options(&paths)));
        // Picking rows, going back and forth, isn't an edit.
        let _ = page.update(Event::Start(Kind::Native, Source::Executable));
        let _ = page.update(Event::Change);
        let _ = page.update(Event::Start(kind, source));
        // Neither is the fetch checkbox, nor its default.
        assert!(page.fetch_checked(true));
        let _ = page.update_with_key_availability(Event::FetchArt(false), true);
        let _ = page.back();
        assert_eq!(page.stage, Stage::Choose);
        assert!(page.take_action().is_none());
        let _ = page.back();
        assert!(!page.leave_prompt, "{kind} {source}");
        assert!(matches!(page.take_action(), Some(Action::Choose)));
    }
}

#[test]
fn a_typed_edit_asks_before_the_choice_page_discards_it() {
    let edits = [
        (
            (Kind::Native, Source::Empty),
            Event::Name("Hades".to_owned()),
        ),
        (
            (Kind::Native, Source::Empty),
            Event::EnvironmentName("hades".to_owned()),
        ),
        (
            (Kind::Native, Source::Executable),
            Event::Path("/games/hades.sh".to_owned()),
        ),
        ((Kind::Native, Source::Executable), Event::Desktop(true)),
        (
            (Kind::Windows, Source::Installer),
            Event::Path("/setup.exe".to_owned()),
        ),
        (
            (Kind::Windows, Source::Empty),
            Event::Template("gaming".to_owned()),
        ),
        (
            (Kind::Windows, Source::Empty),
            Event::Runner("fake".to_owned()),
        ),
        ((Kind::Windows, Source::Empty), Event::Arch(Arch::Win32)),
        (
            (Kind::Windows, Source::Empty),
            Event::Prefix("/prefixes/hades".to_owned()),
        ),
    ];
    for ((kind, source), edit) in edits {
        let mut page = choosing();
        let _ = page.update(Event::Start(kind, source));
        let _ = page.update(edit.clone());
        let _ = page.back();
        assert_eq!(page.stage, Stage::Choose, "{edit:?}");
        assert!(
            !page.leave_prompt,
            "{edit:?}: Details goes back without asking"
        );
        let _ = page.back();
        assert!(page.leave_prompt, "{edit:?}");
        assert!(page.take_action().is_none(), "{edit:?}");
    }
}

#[test]
fn fields_the_picked_row_hides_are_not_edits() {
    // The wine options a Windows row showed, once Native is picked.
    let mut page = page();
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::Arch(Arch::Win32));
    let _ = page.update(Event::Prefix("/prefixes/game".to_owned()));
    assert!(page.has_edits());
    switch(&mut page, Kind::Native, Source::Empty);
    assert!(!page.has_edits());
    let _ = page.back();
    let _ = page.back();
    assert!(!page.leave_prompt);
    assert!(matches!(page.take_action(), Some(Action::Choose)));

    // A path and desktop entry only Existing install shows.
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Native, Source::Empty));
    page.draft.path = "/games/left-over.sh".to_owned();
    page.draft.desktop = true;
    assert!(!page.has_edits());
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Installer));
    page.draft.desktop = true;
    assert!(!page.has_edits());
    // A cleared Environment name override is no override.
    let _ = page.update(Event::EnvironmentName(" ".to_owned()));
    assert!(!page.has_edits());
    // Nor are blank runner and prefix overrides, a blank Name, or a blank
    // installer path.
    let _ = page.update(Event::Runner("  ".to_owned()));
    let _ = page.update(Event::Prefix(" ".to_owned()));
    assert!(!page.has_edits());
    let _ = page.update(Event::Name("   ".to_owned()));
    assert!(!page.has_edits());
    let _ = page.update(Event::Path(" ".to_owned()));
    assert!(!page.has_edits());
}

#[test]
fn another_row_keeps_the_names_and_clears_the_path_and_desktop_entry() {
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Executable));
    let _ = page.update(Event::Name("Hollow Knight".to_owned()));
    let _ = page.update(Event::EditEnvironment);
    let _ = page.update(Event::EnvironmentName("hk".to_owned()));
    let _ = page.update(Event::Path("/games/hk/hk.exe".to_owned()));
    let _ = page.update(Event::Desktop(true));
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Browse);
    let browsing = page.picker_sequence;

    switch(&mut page, Kind::Windows, Source::Installer);
    assert_eq!(page.draft.title, "Hollow Knight");
    assert_eq!(page.draft.environment.as_deref(), Some("hk"));
    assert_eq!(page.draft.path, "");
    assert_eq!(page.draft.desktop, Draft::default().desktop);
    assert_eq!(page.draft.template, "gaming");
    // The program picked for the old row doesn't land in the installer path.
    let _ = page.update(Event::Picked(
        browsing,
        Ok(Some(PathBuf::from("/games/hk/hk.exe"))),
    ));
    assert_eq!(page.draft.path, "");

    // The same row again keeps everything.
    let _ = page.update(Event::Path("/downloads/setup.exe".to_owned()));
    switch(&mut page, Kind::Windows, Source::Installer);
    assert_eq!(page.draft.path, "/downloads/setup.exe");
    assert!(page.has_edits());
}

#[test]
fn the_choice_page_fits_small_dialogs_and_the_window_minimum() {
    use crate::layout_probe::{bounds_at, lay_out, probe, within};
    let mut page = choosing();
    for prompt in [false, true] {
        page.leave_prompt = prompt;
        for (width, height) in [(280.0, 240.0), (400.0, 300.0)] {
            let case = format!("{width}x{height}, prompt {prompt}");
            let size = iced::Size::new(width, height);
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, size);
            assert_eq!(probe.scrollables.len(), 1, "{case}");
            let spilled = probe.outside(iced::Rectangle::with_size(size));
            assert!(spilled.is_empty(), "{case}: {spilled:?}");
            let top = bounds_at(&probe.node, &[0, 0]);
            assert!(within(top, inset), "{case}: {top:?}");
            let (viewport, _) = probe.scrollables[0];
            assert!(viewport.y >= top.y + top.height - 0.01, "{case}");
            assert!(viewport.height > 0.0, "{case}");

            let mut laid = lay_out(page.view_with_key_availability(Palette::DARK, true), size);
            for row in start_rows(&mut laid) {
                assert!(
                    row.x >= inset.x - 0.01 && row.x + row.width <= inset.x + inset.width + 0.01,
                    "{case}: {row:?}"
                );
            }
        }
    }
}

/// `key` pressed on the window, as the event subscription delivers it, held
/// down when `repeat`.
fn pressed(key: iced::keyboard::Key, repeat: bool) -> crate::Message {
    pressed_with(key, iced::keyboard::Modifiers::empty(), repeat)
}

fn pressed_with(
    key: iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    repeat: bool,
) -> crate::Message {
    use iced::keyboard::{Event, Location, key::NativeCode, key::Physical};
    crate::event_message(
        iced::Event::Keyboard(Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat,
        }),
        iced::event::Status::Ignored,
    )
    .unwrap()
}

/// Updates `app` with `message`, then runs the widget work its task asks
/// for and feeds back what that publishes, as the runtime would.
fn update_through(app: &mut crate::App, message: crate::Message) {
    let task = app.update(message);
    let published = {
        let mut laid = crate::layout_probe::lay_out(app.view(), app.window_size);
        crate::more_menu::tests::drive(&mut laid, task)
    };
    for message in published {
        let _ = app.update(message);
    }
}

#[test]
fn a_held_number_key_picks_once() {
    let one = || iced::keyboard::Key::Character("1".into());
    let mut app = crate::tests::app();
    let _ = app.update(crate::Message::Add);
    let _ = app.update(pressed(one(), false));
    assert!(new_page(&app).choosing(), "1 on Add opens New");
    for _ in 0..3 {
        let _ = app.update(pressed(one(), true));
    }
    assert!(new_page(&app).choosing(), "the held 1 walks no further");
    let _ = app.update(pressed(one(), false));
    assert_eq!(
        new_page(&app).details_for(),
        Some((Kind::Windows, Source::Installer)),
        "a new press does"
    );
}

#[test]
fn a_held_esc_walks_no_further_than_its_first_press() {
    let esc = || iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
    let (mut app, generation) = working();
    let _ = done(&mut app, generation, &["game.exe"]);
    assert_eq!(new_page(&app).stage, Stage::Target);
    let _ = app.update(pressed(esc(), false));
    assert_eq!(
        new_page(&app).stage,
        Stage::Finished,
        "Esc skips the target"
    );
    for _ in 0..3 {
        let _ = app.update(pressed(esc(), true));
    }
    assert_eq!(new_page(&app).stage, Stage::Finished, "the held Esc stays");
    let _ = app.update(pressed(esc(), false));
    assert!(app.flow.is_none(), "a new press leaves Finished");

    // The library doesn't get the rest of that hold either.
    app.search = "kept".into();
    update_through(&mut app, pressed(esc(), true));
    assert_eq!(app.search, "kept");
    update_through(&mut app, pressed(esc(), false));
    assert_eq!(app.search, "", "a new press reaches the library");
    // Other keys' repeats still arrive as presses.
    app.search = "kept".into();
    let _ = app.update(pressed(iced::keyboard::Key::Character("a".into()), false));
    update_through(&mut app, pressed(esc(), true));
    assert_eq!(app.search, "");
}

#[test]
fn a_held_esc_on_the_discard_prompt_keeps_it_open() {
    let esc = || iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
    let mut page = page();
    let _ = page.update(Event::Name("Hades".to_owned()));
    let mut app = crate::tests::app();
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    let _ = app.update(pressed(esc(), false));
    assert!(new_page(&app).choosing());
    let _ = app.update(pressed(esc(), false));
    assert!(new_page(&app).leave_prompt_open());
    for _ in 0..3 {
        let _ = app.update(pressed(esc(), true));
    }
    assert!(new_page(&app).leave_prompt_open());
}

#[test]
fn leaving_details_drops_its_running_check_and_picker() {
    for leave in [Event::Back, Event::Change] {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let mut page = offer_page(&paths);
        // An edit whose wait is still pending.
        let _ = page.update(Event::Name("Hades".to_owned()));
        let waiting = page.check_sequence;
        let _ = page.update(leave.clone());
        assert!(
            is_none(page.update(Event::Settled(waiting))),
            "{leave:?}: no check starts off Details"
        );
        let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
        let opened = page.check_sequence;
        assert!(is_none(page.update(Event::Settled(waiting))), "{leave:?}");
        assert_eq!(page.check_sequence, opened, "{leave:?}");

        // A check still running.
        let _ = page.update(Event::Name("Hades 2".to_owned()));
        let _ = page.update(Event::Settled(page.check_sequence));
        let checking = page.check_sequence;
        assert!(page.checking);
        let _ = page.update(Event::Browse);
        let browsing = page.picker_sequence;
        let _ = page.update(leave.clone());
        let shown = |page: &Page| page.summary.as_ref().map(|s| s.draft.title.clone());
        let before = shown(&page);
        let _ = page.update(Event::Checked(
            checking,
            Box::new(page.draft.clone()),
            prepare(&paths, &page.draft).map(Box::new),
        ));
        assert_eq!(shown(&page), before, "{leave:?}: nothing lands off Details");
        let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
        let _ = page.update(Event::Checked(
            checking,
            Box::new(page.draft.clone()),
            prepare(&paths, &page.draft).map(Box::new),
        ));
        assert!(
            page.checking,
            "{leave:?}: only the check Details opened with lands"
        );
        let _ = page.update(Event::Picked(
            browsing,
            Ok(Some(PathBuf::from("/games/hades.exe"))),
        ));
        assert_eq!(page.draft.path, "", "{leave:?}");
    }
}

#[test]
fn details_title_has_change_and_the_choice_page_shows_no_details_error() {
    let size = iced::Size::new(900.0, 720.0);
    let mut page = page();
    assert_eq!(click_label(&page, size, "Change"), ["Change"]);

    let error = "Could not open the file picker";
    page.error = Some(error.to_owned());
    assert_eq!(labeled(&texts(&page, size), error).len(), 1);
    let _ = page.update(Event::Change);
    assert!(labeled(&texts(&page, size), error).is_empty());
}

#[test]
fn a_held_ctrl_enter_submits_once() {
    let ctrl_enter = |repeat| {
        pressed_with(
            iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            iced::keyboard::Modifiers::CTRL,
            repeat,
        )
    };
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Name("Hollow Knight".to_owned()));
    let checking = page.check_sequence;
    let draft = Box::new(page.draft.clone());
    let mut app = app_with_key(page);
    // Pressed while the Summary checks: nothing, and nothing kept for later.
    let _ = app.update(ctrl_enter(false));
    assert_eq!(new_page(&app).stage, Stage::Details);
    // The check lands while the key is still down.
    let _ = app.update(crate::Message::Flow(
        app.flow_generation,
        crate::flow::Event::Create(Box::new(Event::Checked(
            checking,
            draft,
            Ok(Box::new(ready_review())),
        ))),
    ));
    assert!(new_page(&app).checked().is_ok());
    for _ in 0..3 {
        let _ = app.update(ctrl_enter(true));
    }
    assert_eq!(
        new_page(&app).stage,
        Stage::Details,
        "the held chord creates nothing"
    );
    let _ = app.update(ctrl_enter(false));
    assert_eq!(new_page(&app).stage, Stage::Working, "a new press does");
}

#[test]
fn edits_inside_the_wait_run_one_check_and_a_later_edit_runs_another() {
    let mut page = page();
    let waits: Vec<_> = ["H", "Ha", "Had"]
        .into_iter()
        .map(|name| page.update(Event::Name(name.to_owned())))
        .collect();
    assert!(page.checking, "Create waits from the first edit");
    // Each edit's wait is the full delay and names its own check, so a
    // later edit restarts it.
    let settled: Vec<u64> = waits
        .into_iter()
        .map(|task| {
            let (events, waited) = waited(task);
            assert!(waited >= SETTLE, "{waited:?}");
            match events[..] {
                [Event::Settled(sequence)] => sequence,
                _ => panic!("{events:?}"),
            }
        })
        .collect();
    assert_eq!(settled.last(), Some(&page.check_sequence));
    // Only the last one checks; the earlier ones land nothing.
    for sequence in &settled[..2] {
        assert!(is_none(page.update(Event::Settled(*sequence))));
        assert!(page.checking);
    }
    let checks = outputs(page.update(Event::Settled(settled[2])));
    assert!(
        matches!(&checks[..], [Event::Checked(sequence, _, Ok(_))] if *sequence == settled[2]),
        "{checks:?}"
    );
    for event in checks {
        let _ = page.update(event);
    }
    assert!(!page.checking);
    assert_eq!(page.summary.as_ref().unwrap().draft.title, "Had");
    // A wait that lands twice runs nothing more.
    assert!(is_none(page.update(Event::Settled(settled[2]))));

    // An edit after the check runs another.
    let (events, _) = waited(page.update(Event::Name("Hades".to_owned())));
    let [Event::Settled(sequence)] = events[..] else {
        panic!("{events:?}");
    };
    assert_ne!(sequence, settled[2]);
    let checks = outputs(page.update(Event::Settled(sequence)));
    for event in checks {
        let _ = page.update(event);
    }
    assert!(!page.checking);
    assert_eq!(page.summary.as_ref().unwrap().draft.title, "Hades");
}

#[test]
fn a_stale_check_drops_by_its_own_number_not_by_the_generation() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Name("Hades".to_owned()));
    let _ = page.update(Event::Settled(page.check_sequence));
    let started = page.check_sequence;
    let checked = Box::new(page.draft.clone());
    let result = prepare(&paths, &page.draft).map(Box::new);
    // Work that shares the generation, like reloading the options, leaves
    // the check alone.
    let _ = page.load_options();
    let _ = page.update(Event::Checked(started, checked.clone(), result.clone()));
    assert!(!page.checking);
    assert!(page.checked().is_ok(), "{:?}", page.summary);

    // An edit after a check started drops what it finds.
    let _ = page.update(Event::Name("Hades 2".to_owned()));
    let _ = page.update(Event::Settled(page.check_sequence));
    let generation = page.generation;
    let _ = page.update(Event::Checked(started, checked, result));
    assert!(page.checking);
    assert_eq!(page.summary.as_ref().unwrap().draft.title, "Hades");
    assert_eq!(page.generation, generation);
}

#[test]
fn create_waits_for_the_check_a_download_and_valid_fields() {
    let size = iced::Size::new(900.0, 720.0);
    // How often the reason shows: beside Create, and for Checking... on the
    // Summary's heading, and for an error in the Summary.
    type Case = (&'static str, fn(&mut Page), Hold, usize);
    let cases: [Case; 3] = [
        (
            "checking",
            |page| {
                let _ = page.update(Event::Name("Test game 2".to_owned()));
            },
            Hold::Checking,
            2,
        ),
        (
            "missing runner",
            |page| land(page, missing_runner()),
            Hold::Missing("Runner GE-Proton-Latest isn't installed".to_owned()),
            1,
        ),
        (
            "invalid name",
            |page| {
                let _ = page.update(Event::Name("Test\u{7}game".to_owned()));
                let _ = page.update(Event::Settled(page.check_sequence));
            },
            Hold::Problem(Problem::Error(
                "Invalid title: can't contain control characters.".to_owned(),
            )),
            2,
        ),
    ];
    for (label, make, hold, shown) in cases {
        let mut page = summarized(ready_review());
        make(&mut page);
        assert_eq!(page.checked().map(|_| ()), Err(hold.clone()), "{label}");
        let (reason, _) = hold_text(hold, Palette::DARK);
        assert_eq!(
            labeled(&texts(&page, size), &reason).len(),
            shown,
            "{label}"
        );
        assert!(click_label(&page, size, "Create").is_empty(), "{label}");
        let _ = page.update(Event::Create);
        assert_eq!(page.stage, Stage::Details, "{label}");
        assert!(page.token.is_none(), "{label}");
    }

    let mut page = summarized(ready_review());
    assert!(page.checked().is_ok());
    assert_eq!(click_label(&page, size, "Create"), ["Create"]);
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Working);
}

#[test]
fn ctrl_enter_while_checking_does_nothing_and_never_fires_later() {
    let mut page = summarized(ready_review());
    let _ = page.update(Event::Name("Test game 2".to_owned()));
    let generation = page.generation;
    assert!(is_none(page.submit(true)));
    assert_eq!(page.stage, Stage::Details);
    let _ = page.update(Event::Checked(
        page.check_sequence,
        Box::new(page.draft.clone()),
        Ok(Box::new(ready_review())),
    ));
    assert!(page.checked().is_ok());
    assert_eq!(page.stage, Stage::Details, "nothing was kept to run now");
    assert_eq!(page.generation, generation);
    assert!(page.token.is_none());
}

#[test]
fn reasons_for_untouched_fields_are_quiet_until_they_are_touched() {
    let quiet = |reason| Some(Problem::Untouched(reason));
    let loud = |reason: &str| Some(Problem::Error(reason.to_owned()));
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Installer));
    assert_eq!(page.problem(), quiet("Enter a name"));
    let _ = page.update(Event::Name("Hades".to_owned()));
    assert_eq!(page.problem(), quiet("Choose an installer"));
    for (kind, source) in [
        (Kind::Windows, Source::Executable),
        (Kind::Native, Source::Executable),
    ] {
        switch(&mut page, kind, source);
        assert_eq!(page.problem(), quiet("Choose a program"), "{kind} {source}");
    }
    // Typed and cleared, they're errors.
    let _ = page.update(Event::Path("/games/hades.sh".to_owned()));
    let _ = page.update(Event::Path(String::new()));
    assert_eq!(page.problem(), loud("Choose a program"));
    let _ = page.update(Event::Name(String::new()));
    assert_eq!(page.problem(), loud("Enter a name"));
    // A browsed path is touched too, and another row's path starts untouched.
    let _ = page.update(Event::Name("Hades".to_owned()));
    switch(&mut page, Kind::Windows, Source::Installer);
    assert_eq!(page.problem(), quiet("Choose an installer"));
    let _ = page.update(Event::Browse);
    let _ = page.update(Event::Picked(
        page.picker_sequence,
        Ok(Some(PathBuf::from("/setup.exe"))),
    ));
    let _ = page.update(Event::Path(" ".to_owned()));
    assert_eq!(page.problem(), loud("Choose an installer"));
    // An Environment name typed alone is a touched name.
    let mut page = page_with_name_override();
    let _ = page.update(Event::EnvironmentName(String::new()));
    assert_eq!(page.problem(), loud("Enter a name"));

    // The strip says it quietly, as the Summary does.
    let palette = Palette::DARK;
    let shown = |problem| hold_text(Hold::Problem(problem), palette).1;
    assert_eq!(shown(Problem::Untouched("Enter a name")), palette.muted);
    assert_eq!(
        shown(Problem::Error("Enter a name".to_owned())),
        palette.danger
    );
    assert_eq!(hold_text(Hold::Checking, palette).1, palette.muted);
    assert_eq!(
        hold_text(Hold::Missing(String::new()), palette).1,
        palette.warning
    );
}

fn page_with_name_override() -> Page {
    let mut page = page();
    let _ = page.update(Event::EditEnvironment);
    let _ = page.update(Event::EnvironmentName("hades".to_owned()));
    assert_eq!(page.problem(), None);
    page
}

#[test]
fn the_notes_that_change_create_show_beside_it_once_it_is_ready() {
    use crate::layout_probe::probe;
    let size = iced::Size::new(400.0, 300.0);
    let adopted = "This will register the existing prefix in place.";
    let replaced = "Proton's own DXVK and vkd3d-proton replace the template's.";
    let both = format!("{adopted} {replaced}");
    let cases = [
        (true, false, adopted.to_owned()),
        (false, true, replaced.to_owned()),
        (true, true, both.clone()),
    ];
    for (adopts, omits, pinned) in cases {
        let review = Review {
            adopted: adopts,
            omitted_components: omits,
            ..ready_review()
        };
        let page = summarized(review.clone());
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let (viewport, content) = probe(&mut view, size).scrollables[0];
        let texts = texts(&page, size);
        let bounds = labeled(&texts, &pinned);
        assert_eq!(bounds.len(), 1, "{pinned}");
        assert!(
            bounds[0].y >= viewport.y + viewport.height,
            "{pinned} is pinned"
        );
        // The Summary says them in full, in the scrolling body.
        for note in consequences(&review) {
            let bounds = labeled(&texts, note);
            assert_eq!(bounds.len(), 1, "{note}");
            assert!(
                crate::layout_probe::within(bounds[0], content),
                "{note} scrolls"
            );
        }
    }
    let shows = |page: &Page, text: &str| !labeled(&texts(page, size), text).is_empty();
    // While a newer check runs, the strip says so instead.
    let mut page = summarized(adopting());
    let _ = page.update(Event::Name("Test game 2".to_owned()));
    assert!(!shows(&page, &both));
    // And a check that finds neither says nothing beside Create.
    land(&mut page, ready_review());
    for text in [adopted, replaced, both.as_str()] {
        assert!(!shows(&page, text), "{text}");
    }
}

#[test]
fn the_last_summary_stays_in_place_under_checking_while_the_next_check_runs() {
    let size = iced::Size::new(900.0, 720.0);
    let mut page = summarized(missing_runner());
    let summary_part = |texts: Vec<(String, iced::Rectangle)>| -> Vec<(String, iced::Rectangle)> {
        let start = texts
            .iter()
            .position(|(text, _)| text == "Summary")
            .unwrap();
        let end = texts.iter().position(|(text, _)| text == "Create").unwrap();
        texts[start..end].to_vec()
    };
    let before = summary_part(texts(&page, size));
    assert!(before.iter().any(|(text, _)| text == "Test game"));
    let _ = page.update(Event::Name("Something else entirely".to_owned()));
    assert!(page.checking);
    let mut during = summary_part(texts(&page, size));
    // On the heading's line, beside it.
    let checking = during.remove(1);
    assert_eq!(checking.0, "Checking...");
    let heading = during[0].1;
    assert!(checking.1.x > heading.x + heading.width, "{checking:?}");
    assert!(checking.1.y < heading.y + heading.height, "{checking:?}");
    // Everything else is the last check's, where it was.
    assert_eq!(during, before);
    // Its downloads wait for the check. The Summary is below the fold in
    // a short dialog, which a click there would never reach.
    let tall = iced::Size::new(900.0, 4000.0);
    assert!(click_label(&page, tall, "Download GE-Proton").is_empty());
    let _ = page.update(Event::Download(Provider::GeProton));
    assert_eq!(page.stage, Stage::Details);

    // Dimmed: drawn in the muted role, darker than before on a dark theme.
    let brightest = |page: &Page, label: &str| {
        let bounds = last_labeled(&texts(page, size), label);
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let pixels = crate::layout_probe::render(&mut view, size);
        let mut most = 0u32;
        for y in bounds.y as u32..(bounds.y + bounds.height) as u32 {
            for x in bounds.x as u32..(bounds.x + bounds.width) as u32 {
                let at = ((y * size.width as u32 + x) * 4) as usize;
                most = most.max(pixels[at..at + 3].iter().map(|&c| u32::from(c)).sum());
            }
        }
        most
    };
    let dim = brightest(&page, "Test game");
    land(&mut page, missing_runner());
    // The landed check is of the new draft, so its Name row changed.
    let bright = brightest(&page, "Something else entirely");
    assert!(dim < bright, "{dim} {bright}");
    assert_eq!(dimmed(Palette::DARK).foreground, Palette::DARK.muted);
    assert_eq!(dimmed(Palette::DARK).warning, Palette::DARK.muted);
    assert_eq!(dimmed(Palette::DARK).danger, Palette::DARK.muted);

    // The first check has nothing to keep: only Checking... shows.
    let mut page = choosing();
    let _ = page.update(Event::Start(Kind::Windows, Source::Empty));
    let _ = page.update(Event::EditEnvironment);
    page.summary = None;
    let _ = page.update(Event::EnvironmentName("hades".to_owned()));
    let shown = summary_part(texts(&page, size));
    let shown: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(shown, ["Summary", "Checking..."]);
}

#[test]
fn a_check_lands_while_the_discard_prompt_shows() {
    let mut page = summarized(ready_review());
    let _ = page.update(Event::Name("Hades".to_owned()));
    let waiting = page.check_sequence;
    // The prompt opens on Details once Open existing asks (plan 0064 step 6).
    page.leave_prompt = true;
    // Its wait still starts the check, and the check still lands.
    let task = page.update(Event::Settled(waiting));
    assert!(!is_none(task));
    let _ = page.update(Event::Checked(
        waiting,
        Box::new(page.draft.clone()),
        Ok(Box::new(adopting())),
    ));
    assert!(page.leave_prompt);
    assert!(!page.checking);
    assert!(
        page.summary
            .as_ref()
            .unwrap()
            .found
            .as_ref()
            .unwrap()
            .adopted
    );
    // Other events stay blocked.
    let _ = page.update(Event::Name("Hades 2".to_owned()));
    assert_eq!(page.draft.title, "Hades");
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
}

#[test]
fn a_download_and_its_failure_return_to_details_and_check_again() {
    let mut page = summarized(Review {
        runner_row: None,
        prerequisite: Some(Prerequisite::Runner("GE-Proton11-3".to_owned())),
        ..ready_review()
    });
    page.draft.runner = "GE-Proton11-3".to_owned();
    let review = page.summary.clone().unwrap().found.unwrap();
    land(&mut page, review);
    let size = iced::Size::new(900.0, 4000.0);
    assert_eq!(
        click_label(&page, size, "Download GE-Proton"),
        ["Download(GeProton)"]
    );

    // The download itself never runs here; its result is landed below.
    let _ = page.update(Event::Download(Provider::GeProton));
    assert_eq!(page.stage, Stage::Working);
    let checks = page.check_sequence;
    let _ = page.update(Event::Downloaded(
        page.generation,
        Ok("GE-Proton11-4".to_owned()),
    ));
    assert_eq!(page.stage, Stage::Details);
    assert_eq!(
        page.draft.runner, "GE-Proton11-4",
        "the named build installed"
    );
    assert_ne!(page.check_sequence, checks);
    assert!(page.checking);
    assert!(page.error.is_none());

    let review = page.summary.clone().unwrap().found.unwrap();
    land(&mut page, review);
    let _ = page.update(Event::Download(Provider::GeProton));
    assert_eq!(page.stage, Stage::Working);
    let checks = page.check_sequence;
    let _ = page.update(Event::Downloaded(
        page.generation,
        Err("offline".to_owned()),
    ));
    assert_eq!(page.stage, Stage::Details);
    assert_eq!(page.draft.runner, "GE-Proton11-4");
    assert_ne!(page.check_sequence, checks);
    assert!(page.checking);
    assert_eq!(labeled(&texts(&page, size), "offline").len(), 1);
    // A late result from the failed download lands nothing.
    let _ = page.update(Event::Downloaded(
        page.generation.wrapping_sub(1),
        Ok("GE-Proton11-5".to_owned()),
    ));
    assert_eq!(page.draft.runner, "GE-Proton11-4");
}

#[test]
fn back_to_details_from_finished_keeps_the_draft_and_checks_it_again() {
    let size = iced::Size::new(900.0, 720.0);
    let mut page = summarized(ready_review());
    page.report = Some(Report::new());
    page.show_finished();
    assert_eq!(
        click_label(&page, size, "Back to details"),
        ["BackToDetails"]
    );
    let checks = page.check_sequence;
    let _ = page.update(Event::BackToDetails);
    assert_eq!(page.stage, Stage::Details);
    assert_eq!(page.draft.title, "Test game");
    assert!(page.report.is_none());
    assert_ne!(page.check_sequence, checks);
    assert!(page.checking);

    // Not after a creation that registered, which has nothing to go back to.
    let (mut page, _) = registered(false);
    let _ = page.finish();
    assert!(labeled(&texts(&page, size), "Back to details").is_empty());
    let _ = page.update(Event::BackToDetails);
    assert_eq!(page.stage, Stage::Finished);
}

/// Details for a game with the gaming template and the offer showing, its
/// check landed with nothing missing.
fn offering(paths: &Paths) -> Page {
    let mut page = offer_page(paths);
    let _ = page.update(Event::Template("gaming".to_owned()));
    let _ = page.update(Event::Name("Hades".to_owned()));
    assert!(page.offer_shown());
    land(&mut page, ready_review());
    assert!(page.checked().is_ok());
    page
}

#[test]
fn create_waits_for_an_answer_to_the_offer_and_its_recheck() {
    let size = iced::Size::new(900.0, 4000.0);
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offering(&paths);
    let _ = page.update(Event::Offer(true));
    assert!(page.answering);
    // The review was made before the answer, so Create holds and says so,
    // and the Summary shows it's out of date.
    assert_eq!(page.checked().map(|_| ()), Err(Hold::Checking));
    assert_eq!(labeled(&texts(&page, size), "Checking...").len(), 2);
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
    // Even a check landing meanwhile read the old setting.
    land(&mut page, ready_review());
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);

    let before = page.check_sequence;
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        answer_offer(&paths, true),
    ));
    assert!(!page.answering);
    assert_ne!(page.check_sequence, before, "the answer checks again");
    assert_eq!(page.checked().map(|_| ()), Err(Hold::Checking));
    land(&mut page, ready_review());
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Working);
}

#[test]
fn a_failed_answer_to_the_offer_checks_again_and_lets_create_go() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offering(&paths);
    let _ = page.update(Event::Offer(true));
    let before = page.check_sequence;
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        Err("disk full".to_owned()),
    ));
    assert_eq!(page.error.as_deref(), Some("disk full"));
    assert!(!page.answering);
    // It may have been saved before the failure, so the Summary checks.
    assert_ne!(page.check_sequence, before);
    assert!(page.checking);
    land(&mut page, ready_review());
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Working);
}

#[test]
fn an_answer_to_the_offer_lands_while_the_discard_prompt_shows() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    let mut page = offering(&paths);
    let _ = page.update(Event::Offer(true));
    // The prompt opens on Details once Open existing asks (plan 0064 step 6).
    page.leave_prompt = true;
    let before = page.check_sequence;
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        answer_offer(&paths, true),
    ));
    assert!(!page.answering, "Create isn't left waiting for good");
    assert!(page.options.as_ref().is_some_and(|options| !options.offer));
    assert_ne!(page.check_sequence, before);
    let _ = page.update(Event::KeepEditing);
    land(&mut page, ready_review());
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Working);
}

#[test]
fn a_retried_download_clears_the_last_failure() {
    let size = iced::Size::new(900.0, 4000.0);
    let mut page = summarized(missing_runner());
    let _ = page.update(Event::Download(Provider::GeProton));
    let _ = page.update(Event::Downloaded(
        page.generation,
        Err("offline".to_owned()),
    ));
    assert_eq!(page.error.as_deref(), Some("offline"));
    land(&mut page, missing_runner());
    assert_eq!(labeled(&texts(&page, size), "offline").len(), 1);

    let _ = page.update(Event::Download(Provider::GeProton));
    assert_eq!(page.stage, Stage::Working);
    assert!(page.error.is_none(), "the retry is under way");
    let _ = page.update(Event::Downloaded(
        page.generation,
        Ok("GE-Proton11-4".to_owned()),
    ));
    land(&mut page, ready_review());
    assert!(page.error.is_none());
    assert!(labeled(&texts(&page, size), "offline").is_empty());
    assert!(page.checked().is_ok());
}

#[test]
fn create_holds_unless_the_check_was_of_the_draft_as_it_is() {
    // A change that skipped `edited`, as a future edit path might.
    let mut page = summarized(ready_review());
    page.draft.title = "Changed behind its back".to_owned();
    assert!(!page.checking);
    assert_eq!(page.checked().map(|_| ()), Err(Hold::Checking));
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);

    // A check lands with the draft it checked, not the one the page has.
    let mut page = summarized(ready_review());
    let checked = Box::new(page.draft.clone());
    let _ = page.update(Event::Name("Test game 2".to_owned()));
    let _ = page.update(Event::Checked(
        page.check_sequence,
        checked,
        Ok(Box::new(ready_review())),
    ));
    assert!(!page.checking);
    assert_eq!(page.summary.as_ref().unwrap().draft.title, "Test game");
    assert_eq!(page.checked().map(|_| ()), Err(Hold::Checking));
    let _ = page.update(Event::Create);
    assert_eq!(page.stage, Stage::Details);
}

/// Whether anything is drawn on the rows of the text reading `label` in the
/// gutter right of the scrolling body, before the scrollbar. Text that wraps
/// never reaches it, whatever it sits in.
fn ink_beside(page: &Page, size: iced::Size, label: &str) -> bool {
    use crate::layout_probe::probe;
    let bounds = last_labeled(&texts(page, size), label);
    let mut view = page.view_with_key_availability(Palette::DARK, true);
    let (viewport, _) = probe(&mut view, size).scrollables[0];
    let pixels = crate::layout_probe::render(&mut view, size);
    let body_right = (viewport.x + viewport.width - super::frame::SCROLL_GUTTER).ceil() as u32;
    (bounds.y as u32..(bounds.y + bounds.height) as u32).any(|y| {
        (body_right + 1..body_right + 5).any(|x| {
            let at = ((y * size.width as u32 + x) * 4) as usize;
            pixels[at + 3] > 0
        })
    })
}

/// A long word that doesn't wrap lays out no wider than its room but draws
/// past it, so what tells it apart is ink beside its box.
#[test]
fn long_unbroken_names_and_errors_wrap_in_small_dialogs() {
    use crate::layout_probe::probe;
    let runner = format!("GE-Proton-{}", "x".repeat(80));
    let build = format!("v{}", "9".repeat(80));
    let error = format!("https://example.invalid/{}", "a".repeat(120));
    let cases = [
        (
            Review {
                runner_row: None,
                prerequisite: Some(Prerequisite::Runner(runner.clone())),
                ..ready_review()
            },
            format!(
                "Runner {runner} is unavailable. Choose a provider to download its latest release:"
            ),
        ),
        (
            Review {
                prerequisite: Some(Prerequisite::Component {
                    provider: Provider::Dxvk,
                    version: Some(build.clone()),
                }),
                ..ready_review()
            },
            format!(
                "Missing {} exact build {build}. Download it before creating this environment?",
                Provider::Dxvk
            ),
        ),
    ];
    for (review, note) in cases {
        let mut page = summarized(review);
        page.error = Some(error.clone());
        for (width, height) in [(280.0, 240.0), (400.0, 300.0)] {
            let size = iced::Size::new(width, height);
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, size);
            let spilled = probe.outside(iced::Rectangle::with_size(size));
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
            // Tall enough to draw the whole body; the width decides wrapping.
            let tall = iced::Size::new(width, 4000.0);
            for long in [note.as_str(), error.as_str()] {
                assert!(!ink_beside(&page, tall, long), "{width}: {long:.30}");
            }
        }
    }
}

#[test]
fn canceling_a_download_returns_to_details_and_checks_again() {
    let esc = || iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
    for how in ["Cancel", "Back", "Esc", "Force stop"] {
        let mut page = summarized(missing_runner());
        let _ = page.update(Event::Download(Provider::GeProton));
        assert_eq!(page.stage, Stage::Working, "{how}");
        assert!(page.busy(), "{how}");
        let token = page.token.clone().unwrap();
        // The download itself never runs here; what it returns is landed.
        let page = match how {
            "Cancel" => {
                let _ = page.update(Event::Cancel);
                page
            }
            "Back" => {
                let _ = page.back();
                page
            }
            "Esc" => {
                let mut app = crate::tests::app();
                app.flow = Some(crate::flow::Page::New(Box::new(page)));
                let _ = app.update(pressed(esc(), false));
                let Some(crate::flow::Page::New(page)) = app.flow.take() else {
                    panic!("Esc left the New page");
                };
                *page
            }
            _ => {
                let _ = page.update(Event::Cancel);
                let size = iced::Size::new(900.0, 720.0);
                assert_eq!(click_label(&page, size, "Force stop"), ["Force"]);
                let _ = page.update(Event::Force);
                assert!(token.is_forced());
                page
            }
        };
        let mut page = page;
        assert!(token.is_canceled(), "{how}");
        assert_eq!(
            page.stage,
            Stage::Working,
            "{how}: until the download stops"
        );
        let checks = page.check_sequence;
        let _ = page.update(Event::Downloaded(
            page.generation,
            Err("Download canceled".to_owned()),
        ));
        assert_eq!(page.stage, Stage::Details, "{how}");
        assert!(page.token.is_none(), "{how}");
        // A cancel the user chose is no error line under the Summary.
        assert!(page.error.is_none(), "{how}");
        assert!(
            labeled(
                &texts(&page, iced::Size::new(900.0, 720.0)),
                "Download canceled"
            )
            .is_empty(),
            "{how}"
        );
        assert_ne!(page.check_sequence, checks, "{how}");
        assert!(page.checking, "{how}");
        land(&mut page, missing_runner());
        assert_eq!(
            page.checked().map(|_| ()),
            Err(Hold::Missing(
                "Runner GE-Proton-Latest isn't installed".to_owned()
            )),
            "{how}: the download is offered again"
        );
    }
}

// Plan 0064 step 6: the remaining fixes.

/// Which of Working's stop buttons `page` shows.
fn stop_buttons(page: &Page) -> Vec<&'static str> {
    let shown = texts(page, iced::Size::new(900.0, 720.0));
    ["Cancel", "Force stop", "Skip"]
        .into_iter()
        .filter(|label| !labeled(&shown, label).is_empty())
        .collect()
}

/// The target step, reached from a registered environment with
/// `programs` installed programs to pick from.
fn on_target(fetch: bool, programs: usize) -> (Page, Calls) {
    let (mut page, calls) = registered(fetch);
    let report = page.report.as_mut().unwrap();
    report.offer_programs = true;
    report.programs = (0..programs)
        .map(|index| {
            (
                format!("Program {index}"),
                format!("C:\\Games\\Game\\program-{index}.exe"),
            )
        })
        .collect();
    page.stage = Stage::Working;
    let report = page.report.clone().unwrap();
    let _ = page.update(Event::Done(page.generation, report));
    assert_eq!(page.stage, Stage::Target);
    (page, calls)
}

#[test]
fn cancel_shows_only_where_it_works() {
    // Creation sets a token, so it can be canceled, then forced.
    let mut creating = summarized(ready_review());
    let _ = creating.update_with_key_availability(Event::Create, false);
    assert_eq!(creating.stage, Stage::Working);
    assert_eq!(stop_buttons(&creating), ["Cancel"]);
    let _ = creating.update(Event::Cancel);
    assert_eq!(stop_buttons(&creating), ["Force stop"]);

    // So does a download.
    let mut downloading = summarized(missing_runner());
    let _ = downloading.update(Event::Download(Provider::GeProton));
    assert_eq!(downloading.stage, Stage::Working);
    assert_eq!(stop_buttons(&downloading), ["Cancel"]);

    // Add target sets none, so there's nothing for Cancel or Esc to stop.
    let (mut adding, _) = on_target(false, 1);
    adding.target_path = "C:\\Games\\Game\\program-0.exe".to_owned();
    let _ = adding.update(Event::AddTarget);
    assert_eq!(adding.stage, Stage::Working);
    assert_eq!(adding.status, "Adding target...");
    assert_eq!(stop_buttons(&adding), Vec::<&str>::new());
    let status = adding.status.clone();
    let _ = adding.back();
    assert_eq!(adding.status, status, "Esc has nothing to cancel either");

    // Neither does Clean up installer.
    let (mut cleaning, _) = registered(false);
    cleaning.report.as_mut().unwrap().cleanup_resolved = false;
    let _ = cleaning.finish();
    assert_eq!(cleaning.stage, Stage::Finished);
    let _ = cleaning.update(Event::CleanUp);
    assert_eq!(cleaning.stage, Stage::Working);
    assert_eq!(cleaning.status, "Cleaning up installer...");
    assert_eq!(stop_buttons(&cleaning), Vec::<&str>::new());

    // The art fetch keeps its Skip.
    let (mut fetching, _) = registered(true);
    let _ = fetching.finish();
    assert!(fetching.fetching);
    assert_eq!(stop_buttons(&fetching), ["Skip"]);
}

#[test]
fn a_canceled_download_shows_no_error_but_a_failed_one_does() {
    let size = iced::Size::new(900.0, 720.0);
    let mut page = summarized(missing_runner());
    let _ = page.update(Event::Download(Provider::GeProton));
    let _ = page.update(Event::Cancel);
    let _ = page.update(Event::Downloaded(
        page.generation,
        Err("Download canceled".to_owned()),
    ));
    assert_eq!(page.stage, Stage::Details);
    assert!(page.error.is_none());
    assert!(labeled(&texts(&page, size), "Download canceled").is_empty());

    let mut page = summarized(missing_runner());
    let _ = page.update(Event::Download(Provider::GeProton));
    let _ = page.update(Event::Downloaded(
        page.generation,
        Err("offline".to_owned()),
    ));
    assert_eq!(labeled(&texts(&page, size), "offline").len(), 1);
}

/// A page on Details whose environment name is already taken.
fn page_with_existing_environment(name: &str) -> Page {
    let page = page();
    let config = page.paths.environment_config(name);
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\n").unwrap();
    page
}

fn settings_for(name: &str) -> String {
    format!("Settings {{ name: {name:?}, launch: false }}")
}

#[test]
fn open_existing_with_edits_asks_before_dropping_the_draft() {
    let mut page = page_with_existing_environment("hades");
    let _ = page.update(Event::Name("Hades".to_owned()));
    // The name alone leaves at once, so something else is what makes it ask.
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::OpenExisting);
    assert_eq!(page.stage, Stage::Details);
    assert!(page.leave_prompt, "a runner beside the name is an edit");
    assert!(page.take_action().is_none(), "nothing left yet");

    // Keep editing keeps the draft, and the way out is forgotten.
    let _ = page.update(Event::KeepEditing);
    assert!(!page.leave_prompt);
    assert_eq!(page.draft.title, "Hades");
    assert!(page.take_action().is_none());
    // Esc is Back, which answers the prompt like Keep editing.
    let _ = page.update(Event::OpenExisting);
    assert!(page.leave_prompt);
    let _ = page.update(Event::Back);
    assert!(!page.leave_prompt);
    assert_eq!(page.stage, Stage::Details, "Esc stays on Details");
    assert!(page.take_action().is_none());

    // Discard goes to that environment's Settings, not the Add choices.
    let _ = page.update(Event::OpenExisting);
    let _ = page.update(Event::Discard);
    assert!(!page.leave_prompt);
    assert_eq!(
        format!("{:?}", page.take_action().unwrap()),
        settings_for("hades")
    );
}

#[test]
fn a_discard_from_the_choice_page_still_goes_to_the_add_choices() {
    let mut page = page_with_existing_environment("hades");
    let _ = page.update(Event::Name("Hades".to_owned()));
    // The name alone leaves at once, so something else is what makes it ask.
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::OpenExisting);
    let _ = page.update(Event::KeepEditing);
    // Back from the choice page asks too, and its Discard isn't Settings.
    let _ = page.update(Event::Change);
    let _ = page.update(Event::Back);
    assert!(page.leave_prompt);
    let _ = page.update(Event::Discard);
    assert_eq!(format!("{:?}", page.take_action().unwrap()), "Choose");
}

#[test]
fn open_existing_ignores_the_names_that_found_it_but_asks_about_other_edits() {
    // Typing the name, or the environment name, and nothing else leaves at once.
    for by_override in [false, true] {
        let mut page = page_with_existing_environment("wine-favorite-only");
        let _ = if by_override {
            let _ = page.update(Event::EditEnvironment);
            page.update(Event::EnvironmentName("wine-favorite-only".to_owned()))
        } else {
            page.update(Event::Name("wine favorite only".to_owned()))
        };
        assert!(page.has_edits(), "Back on the choice page still asks");
        let _ = page.update(Event::OpenExisting);
        assert!(!page.leave_prompt, "{by_override}");
        assert_eq!(
            format!("{:?}", page.take_action().unwrap()),
            settings_for("wine-favorite-only"),
            "{by_override}"
        );
    }

    // Any other edit makes it ask, one at a time.
    let edits: [(&str, Event); 4] = [
        ("runner", Event::Runner("fake".to_owned())),
        ("arch", Event::Arch(Arch::Win32)),
        ("prefix", Event::Prefix("/somewhere".to_owned())),
        ("template", Event::Template("gaming".to_owned())),
    ];
    for (label, edit) in edits {
        let mut page = page_with_existing_environment("wine-favorite-only");
        let _ = page.update(Event::Name("wine favorite only".to_owned()));
        let _ = page.update(edit);
        let _ = page.update(Event::OpenExisting);
        assert!(page.leave_prompt, "{label}");
        assert!(page.take_action().is_none(), "{label}");
    }
    // A typed path, on a row that has one.
    let mut page = page_with_existing_environment("wine-favorite-only");
    switch(&mut page, Kind::Windows, Source::Executable);
    let _ = page.update(Event::Name("wine favorite only".to_owned()));
    let _ = page.update(Event::Path("/games/game.exe".to_owned()));
    let _ = page.update(Event::OpenExisting);
    assert!(page.leave_prompt, "path");
}

#[test]
fn a_check_lands_while_open_existing_asks() {
    let mut page = page_with_existing_environment("hades");
    let _ = page.update(Event::Name("Hades".to_owned()));
    // The name alone leaves at once, so something else is what makes it ask.
    let _ = page.update(Event::Runner("fake".to_owned()));
    let waiting = page.check_sequence;
    let _ = page.update(Event::OpenExisting);
    assert!(page.leave_prompt);
    // Its wait still starts the check, and the check still lands.
    let task = page.update(Event::Settled(waiting));
    assert!(
        is_none(task),
        "the name is taken, so there's nothing to run"
    );
    let _ = page.update(Event::Checked(
        waiting,
        Box::new(page.draft.clone()),
        Ok(Box::new(adopting())),
    ));
    assert!(page.leave_prompt);
    assert!(!page.checking);
    let _ = page.update(Event::KeepEditing);
    assert!(
        page.summary
            .as_ref()
            .unwrap()
            .found
            .as_ref()
            .unwrap()
            .adopted
    );
    assert!(
        page.checked().is_ok(),
        "the landed check is what Create sees"
    );
}

#[test]
fn the_architecture_choice_clears_the_override() {
    // The list's choices publish what the page needs.
    assert_eq!(format!("{:?}", ArchPick::Default.event()), "ArchDefault");
    assert_eq!(
        format!("{:?}", ArchPick::Arch(Arch::Win32).event()),
        "Arch(Win32)"
    );
    assert_eq!(
        ArchPick::Default.to_string(),
        "Use template or global default"
    );

    let mut page = page();
    assert!(!page.has_edits());
    let _ = page.update(Event::Arch(Arch::Win32));
    assert_eq!(page.draft.arch, Some(Arch::Win32));
    assert!(page.has_edits());
    // Back to unset is the default again, so it's no edit.
    let _ = page.update(Event::ArchDefault);
    assert_eq!(page.draft.arch, None);
    assert!(!page.has_edits());
}

#[test]
fn the_summary_shows_the_template_or_global_architecture_once_cleared() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    let global = GlobalConfig::load(&paths).unwrap().defaults.arch;
    let other = if global == Arch::Win64 {
        Arch::Win32
    } else {
        Arch::Win64
    };
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Name("Hollow Knight".to_owned()));
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::Arch(other));
    settle(&mut page);
    let arch = |page: &Page| page.checked().map(|review| review.arch);
    assert_eq!(arch(&page), Ok(other), "{:?}", page.summary);
    let _ = page.update(Event::ArchDefault);
    settle(&mut page);
    assert_eq!(arch(&page), Ok(global));
    let shown = texts(&page, iced::Size::new(900.0, 4000.0));
    assert!(
        !labeled(&shown, &global.to_string()).is_empty(),
        "{shown:?}"
    );
}

/// The pixels of the Environment name override's box on a page whose
/// override was cleared after typing `title`.
fn cleared_override_box(title: &str) -> Vec<u8> {
    let size = iced::Size::new(600.0, 900.0);
    let mut page = page();
    let _ = page.update(Event::Name(title.to_owned()));
    let _ = page.update(Event::EditEnvironment);
    let _ = page.update(Event::EnvironmentName("typed".to_owned()));
    let _ = page.update(Event::EnvironmentName(String::new()));
    assert_eq!(page.draft.environment.as_deref(), Some(""));
    let label = last_labeled(&texts(&page, size), "Environment name");
    let mut view = page.view_with_key_availability(Palette::DARK, true);
    let pixels = crate::layout_probe::render(&mut view, size);
    // The input sits right under its label.
    let (top, rows) = ((label.y + label.height) as usize, 30);
    let (left, columns) = (label.x as usize, 260);
    let stride = size.width as usize * 4;
    (top..top + rows)
        .flat_map(|y| pixels[y * stride + left * 4..y * stride + (left + columns) * 4].to_vec())
        .collect()
}

#[test]
fn a_cleared_override_shows_the_derived_name_as_its_placeholder() {
    assert_eq!(
        Draft {
            title: "Hollow Knight".to_owned(),
            ..Draft::default()
        }
        .environment_placeholder(),
        "hollow-knight"
    );
    assert_eq!(
        Draft::default().environment_placeholder(),
        "Environment name"
    );

    // The box is drawn with it: two titles give two boxes, and the same
    // title gives the same one.
    let knight = cleared_override_box("Hollow Knight");
    assert_eq!(knight, cleared_override_box("Hollow Knight"));
    assert_ne!(knight, cleared_override_box("Hades"));
}

#[test]
fn the_key_strings_are_neutral_where_they_are_not_about_games() {
    let size = iced::Size::new(900.0, 720.0);
    assert_eq!(NAME_HINT, "Name");

    // The target step, its error, its working status, and Finished.
    let (mut page, _) = on_target(false, 1);
    let shown = texts(&page, size);
    for expected in [
        "Choose a target",
        "Choose the program to run. The installer itself is not a target.",
    ] {
        assert_eq!(labeled(&shown, expected).len(), 1, "{expected}");
    }
    let _ = page.update(Event::AddTarget);
    assert_eq!(labeled(&texts(&page, size), "Choose a program").len(), 1);
    page.target_path = "C:\\Games\\Game\\program-0.exe".to_owned();
    let _ = page.update(Event::AddTarget);
    assert_eq!(labeled(&texts(&page, size), "Adding target...").len(), 1);

    let (mut page, _) = registered(false);
    page.report.as_mut().unwrap().playable = true;
    let _ = page.finish();
    assert_eq!(
        labeled(&texts(&page, size), "Launch it when this page closes").len(),
        1
    );

    // Nothing in what a new environment shows says "game" where it isn't one.
    for page in [page, summarized(ready_review())] {
        for (text, _) in texts(&page, size) {
            for old in ["game target", "game executable", "Launch game", "Game name"] {
                assert!(!text.contains(old), "{text}");
            }
        }
    }
}

/// Esc, as the window delivers it.
fn esc() -> iced::keyboard::Key {
    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
}

fn app_on(page: Page) -> crate::App {
    let mut app = crate::tests::app();
    app.flow = Some(crate::flow::Page::New(Box::new(page)));
    app
}

#[test]
fn esc_on_the_choice_page_leaves_for_the_add_choices() {
    let mut app = app_on(choosing());
    let _ = app.update(pressed(esc(), false));
    assert!(matches!(app.flow, Some(crate::flow::Page::Choose(_))));
}

#[test]
fn esc_on_details_goes_back_to_the_choice_page_keeping_the_draft() {
    let mut page = page();
    let _ = page.update(Event::Name("Hades".to_owned()));
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    let page = new_page(&app);
    assert!(page.choosing());
    assert_eq!(page.draft.title, "Hades");
}

#[test]
fn esc_answers_the_discard_prompt_on_the_choice_page_and_on_details() {
    // On the choice page, Esc opens the prompt and the next one closes it.
    let mut page = page();
    let _ = page.update(Event::Name("Hades".to_owned()));
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    let _ = app.update(pressed(esc(), false));
    assert!(new_page(&app).leave_prompt_open());
    let _ = app.update(pressed(esc(), false));
    let page = new_page(&app);
    assert!(!page.leave_prompt_open());
    assert!(page.choosing());
    assert_eq!(page.draft.title, "Hades");

    // On Details, where Open existing opens it, Esc closes it and stays.
    let mut page = page_with_existing_environment("hades");
    let _ = page.update(Event::Name("Hades".to_owned()));
    // The name alone leaves at once, so something else is what makes it ask.
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::OpenExisting);
    let mut app = app_on(page);
    assert!(new_page(&app).leave_prompt_open());
    let _ = app.update(pressed(esc(), false));
    let page = new_page(&app);
    assert!(!page.leave_prompt_open());
    assert_eq!(page.details_for(), Some((Kind::Windows, Source::Empty)));
}

#[test]
fn esc_during_the_fetch_skips_it() {
    let (mut page, _) = registered(true);
    let _ = page.finish();
    let token = page.fetch_token.clone().unwrap();
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    assert!(token.is_canceled());
    assert_eq!(new_page(&app).status, "Skipping artwork...");
}

#[test]
fn esc_on_target_skips_the_target_and_starts_the_fetch() {
    let (page, _) = on_target(true, 1);
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    let page = new_page(&app);
    assert_eq!(page.stage, Stage::Working);
    assert!(page.fetching && page.art_started);

    // Without the fetch, it goes straight to Finished.
    let (page, _) = on_target(false, 1);
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    assert_eq!(new_page(&app).stage, Stage::Finished);
}

#[test]
fn esc_on_finished_leaves_for_the_library() {
    let (mut page, _) = registered(false);
    let _ = page.finish();
    assert_eq!(page.stage, Stage::Finished);
    let mut app = app_on(page);
    let _ = app.update(pressed(esc(), false));
    assert!(app.flow.is_none());
}

/// Target's controls, found by their labels, in a dialog of `size`: Back
/// alone above the scrolling content, Confirm and Skip for now pinned under
/// it at the left.
fn assert_target_shape(page: &Page, size: iced::Size, case: &str) {
    use crate::layout_probe::{probe, within};
    let inset = iced::Rectangle::new(
        iced::Point::new(24.0, 24.0),
        iced::Size::new(size.width - 48.0, size.height - 48.0),
    );
    let mut view = page.view_with_key_availability(Palette::DARK, true);
    let probe = probe(&mut view, size);
    let spilled = probe.outside(iced::Rectangle::with_size(size));
    assert!(spilled.is_empty(), "{case}: {spilled:?}");
    assert_eq!(probe.scrollables.len(), 1, "{case}");
    let (viewport, _) = probe.scrollables[0];
    assert!(viewport.height > 0.0, "{case}: {viewport:?}");
    let shown = texts(page, size);
    let back = last_labeled(&shown, "Back");
    assert!(within(back, inset), "{case}: {back:?}");
    assert!(back.y + back.height <= viewport.y, "{case}: {back:?}");
    assert!(back.y < inset.y + 32.0, "{case}: Back at the top, {back:?}");
    let title = last_labeled(&shown, "Choose a target");
    assert!(
        title.y >= viewport.y,
        "{case}: the title scrolls, {title:?}"
    );
    let confirm = last_labeled(&shown, "Confirm");
    let skip = last_labeled(&shown, "Skip for now");
    for (label, bounds) in [("Confirm", confirm), ("Skip for now", skip)] {
        assert!(within(bounds, inset), "{case}: {label}: {bounds:?}");
        assert!(
            bounds.y >= viewport.y + viewport.height,
            "{case}: {label} is pinned, {bounds:?}"
        );
    }
    // The page is centered once the dialog is wider than its content.
    let left = ((size.width - frame::PAGE_MAX_WIDTH) / 2.0).max(inset.x);
    assert!(
        confirm.x - left < 30.0,
        "{case}: Confirm at the left, {confirm:?}"
    );
    assert!(
        skip.x >= confirm.x + confirm.width || skip.y >= confirm.y + confirm.height,
        "{case}: Skip for now beside or under Confirm, {skip:?}"
    );
}

#[test]
fn target_has_back_alone_at_the_top_and_its_actions_pinned_at_the_bottom_left() {
    for programs in [0, 1, 40] {
        let (page, _) = on_target(false, programs);
        for (width, height) in [(280.0, 240.0), (400.0, 300.0), (900.0, 720.0)] {
            let size = iced::Size::new(width, height);
            assert_target_shape(
                &page,
                size,
                &format!("{programs} programs at {width}x{height}"),
            );
        }
        let size = iced::Size::new(900.0, 720.0);
        assert_eq!(click_label(&page, size, "Back"), ["Back"]);
        assert_eq!(click_label(&page, size, "Skip for now"), ["SkipTarget"]);
        // Confirm is off until there's a program, and then it publishes.
        assert!(click_label(&page, size, "Confirm").is_empty());
        let mut page = page;
        page.target_path = "C:\\Games\\Game\\program-0.exe".to_owned();
        assert_target_shape(&page, size, "with a path");
        assert_eq!(click_label(&page, size, "Confirm"), ["AddTarget"]);
    }
}

#[test]
fn back_on_target_skips_the_target_and_the_fetch_runs_once_after_it() {
    let (mut page, calls) = on_target(true, 1);
    assert_eq!(call_count(&calls), 0);
    let size = iced::Size::new(900.0, 720.0);
    // The click publishes Back, which does what Esc does.
    assert_eq!(click_label(&page, size, "Back"), ["Back"]);
    let task = page.update(Event::Back);
    assert!(page.fetching);
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 1);
    assert_eq!(page.stage, Stage::Finished);

    // Without a fetch, Back goes to Finished at once, with no target added.
    let (mut page, calls) = on_target(false, 1);
    let _ = page.update(Event::Back);
    assert_eq!(page.stage, Stage::Finished);
    assert_eq!(call_count(&calls), 0);
    assert!(!page.report.as_ref().unwrap().playable);
}

#[test]
fn the_fetch_and_finished_pages_fit_small_dialogs() {
    use crate::layout_probe::probe;
    let (mut fetching, _) = registered(true);
    // The creation log stays under the fetch's status.
    for line in 0..40 {
        let _ = fetching.append_log(&format!("Installer line {line} with some words to wrap\n"));
    }
    let _ = fetching.finish();
    assert!(fetching.fetching);
    let (mut finished, _) = registered(false);
    let report = finished.report.as_mut().unwrap();
    report.playable = true;
    report.notes = vec!["A note about the creation that runs long ".repeat(4)];
    let _ = finished.finish();
    assert_eq!(finished.stage, Stage::Finished);
    for (label, page) in [("fetch", &fetching), ("finished", &finished)] {
        for (width, height) in [(280.0, 240.0), (400.0, 300.0)] {
            let size = iced::Size::new(width, height);
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, size);
            assert!(!probe.scrollables.is_empty(), "{label} at {width}x{height}");
            let spilled = probe.outside(iced::Rectangle::with_size(size));
            assert!(
                spilled.is_empty(),
                "{label} at {width}x{height}: {spilled:?}"
            );
        }
    }
}

/// `page` on Target after an add that failed with `error`.
fn failed_add(error: &str) -> Page {
    let (mut page, _) = on_target(false, 3);
    page.target_path = "C:\\Games\\Game\\program-0.exe".to_owned();
    let _ = page.update(Event::AddTarget);
    assert_eq!(page.stage, Stage::Working);
    let _ = page.update(Event::TargetAdded(page.generation, Err(error.to_owned())));
    assert_eq!(page.stage, Stage::Target);
    page
}

#[test]
fn target_confirm_waits_for_a_path_and_its_reasons_stay_in_the_pinned_strip() {
    use crate::layout_probe::{probe, within};
    let long = format!("Cannot run {}", "/a-long-folder-name".repeat(8));
    let (empty, _) = on_target(false, 40);
    let failed = failed_add(&long);
    let cut_error = cut(long.clone());
    assert!(cut_error.ends_with("..."));
    for (label, page, reason, danger) in [
        ("empty", &empty, "Choose a program", false),
        ("failed", &failed, cut_error.as_str(), true),
    ] {
        for (width, height) in [(280.0, 240.0), (400.0, 300.0), (900.0, 720.0)] {
            let case = format!("{label} at {width}x{height}");
            let size = iced::Size::new(width, height);
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_with_key_availability(Palette::DARK, true);
            let probe = probe(&mut view, size);
            let (viewport, _) = probe.scrollables[0];
            assert!(viewport.height > 0.0, "{case}: {viewport:?}");
            let shown = texts(page, size);
            // Once, and in the strip under the scrolling content.
            assert_eq!(labeled(&shown, reason).len(), 1, "{case}: {shown:?}");
            let bounds = last_labeled(&shown, reason);
            assert!(within(bounds, inset), "{case}: {bounds:?}");
            assert!(
                bounds.y >= viewport.y + viewport.height,
                "{case}: {reason} is pinned, {bounds:?}"
            );
            let confirm = last_labeled(&shown, "Confirm");
            assert!(within(confirm, inset), "{case}: {confirm:?}");
            let skip = last_labeled(&shown, "Skip for now");
            assert!(within(skip, inset), "{case}: {skip:?}");
        }
        let size = iced::Size::new(900.0, 720.0);
        // Off with no path; a failed add left the path, so Confirm retries.
        assert_eq!(
            click_label(page, size, "Confirm").is_empty(),
            !danger,
            "{label}"
        );
    }
}

#[test]
fn a_failed_adds_error_goes_when_target_is_left_or_retried() {
    let size = iced::Size::new(900.0, 720.0);
    let error = "Cannot run it";
    for leave in ["Skip", "Back", "Esc", "Retry"] {
        let mut page = failed_add(error);
        assert_eq!(labeled(&texts(&page, size), error).len(), 1, "{leave}");
        match leave {
            "Skip" => drop(page.update(Event::SkipTarget)),
            "Back" => drop(page.update(Event::Back)),
            "Esc" => {
                let mut app = app_on(page);
                let _ = app.update(pressed(esc(), false));
                let Some(crate::flow::Page::New(boxed)) = app.flow.take() else {
                    panic!("Esc left the New page");
                };
                page = *boxed;
            }
            _ => drop(page.update(Event::AddTarget)),
        }
        assert!(page.error.is_none(), "{leave}");
        assert!(labeled(&texts(&page, size), error).is_empty(), "{leave}");
        if leave != "Retry" {
            assert_eq!(page.stage, Stage::Finished, "{leave}");
        }
    }
}

#[test]
fn target_actions_wrap_when_they_run_out_of_room() {
    use crate::layout_probe::probe;
    let (mut page, _) = on_target(false, 1);
    page.target_path = "C:\\Games\\Game\\program-0.exe".to_owned();
    // Narrower than any dialog, so the row has to wrap to stay inside.
    let size = iced::Size::new(200.0, 400.0);
    let mut view = page.view_with_key_availability(Palette::DARK, true);
    let spilled = probe(&mut view, size).outside(iced::Rectangle::with_size(size));
    assert!(spilled.is_empty(), "{spilled:?}");
    let shown = texts(&page, size);
    let confirm = last_labeled(&shown, "Confirm");
    let skip = last_labeled(&shown, "Skip for now");
    assert!(skip.y >= confirm.y + confirm.height, "{confirm:?} {skip:?}");
}

#[test]
fn the_architecture_default_follows_a_template_that_sets_one() {
    let root = tempfile::tempdir().unwrap();
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
    fake_runner(&paths);
    fs::create_dir_all(paths.templates_dir()).unwrap();
    fs::write(paths.templates_dir().join("old.toml"), "arch = \"win32\"\n").unwrap();
    let mut page = offer_page(&paths);
    let _ = page.update(Event::Options(read_options(&paths)));
    let _ = page.update(Event::Name("Hollow Knight".to_owned()));
    let _ = page.update(Event::Runner("fake".to_owned()));
    let _ = page.update(Event::Template("old".to_owned()));
    let _ = page.update(Event::Arch(Arch::Win64));
    settle(&mut page);
    let arch = |page: &Page| page.checked().map(|review| review.arch);
    assert_eq!(arch(&page), Ok(Arch::Win64), "{:?}", page.summary);
    let _ = page.update(Event::ArchDefault);
    settle(&mut page);
    assert_eq!(
        arch(&page),
        Ok(Arch::Win32),
        "the template's, not the global"
    );
}

#[test]
fn a_desktop_entry_choice_is_an_edit_open_existing_asks_about() {
    let mut page = page_with_existing_environment("wine-favorite-only");
    switch(&mut page, Kind::Windows, Source::Executable);
    let _ = page.update(Event::Name("wine favorite only".to_owned()));
    let _ = page.update(Event::OpenExisting);
    assert!(!page.leave_prompt, "the name alone leaves");
    let _ = page.take_action();
    let _ = page.update(Event::Desktop(true));
    let _ = page.update(Event::OpenExisting);
    assert!(page.leave_prompt);
    assert!(page.take_action().is_none());
}

#[test]
fn back_and_esc_on_target_after_clean_up_never_fetch_again() {
    for esc_key in [false, true] {
        let (mut page, calls) = registered(true);
        page.draft.source = Source::Installer;
        page.report.as_mut().unwrap().cleanup_resolved = false;
        page.stage = Stage::Working;
        let report = page.report.clone().unwrap();
        let task = page.update(Event::Done(page.generation, report));
        let _ = page.update(Event::SkipFetch);
        drive(&mut page, task, true);
        assert_eq!(page.stage, Stage::Finished);
        assert_eq!(call_count(&calls), 1);

        let _ = page.update(Event::CleanUp);
        let _ = page.update(Event::Cleaned(page.generation, Ok(true)));
        assert_eq!(page.stage, Stage::Target);
        if esc_key {
            let mut app = app_on(page);
            let _ = app.update(pressed(esc(), false));
            let Some(crate::flow::Page::New(boxed)) = app.flow.take() else {
                panic!("Esc left the New page");
            };
            page = *boxed;
        } else {
            let task = page.update(Event::Back);
            drive(&mut page, task, true);
        }
        assert!(!page.fetching, "{esc_key}");
        assert_eq!(page.stage, Stage::Finished, "{esc_key}");
        assert_eq!(call_count(&calls), 1, "{esc_key}");
    }
}

/// The pixel at (`x`, `y`) of `render`'s RGBA output, as `[r, g, b, a]`.
fn pixel_at(pixels: &[u8], width: u32, x: f32, y: f32) -> [u8; 4] {
    let at = (y as usize * width as usize + x as usize) * 4;
    pixels[at..at + 4].try_into().unwrap()
}

fn rgba8(color: iced::Color) -> [u8; 4] {
    color.into_rgba8()
}

#[test]
fn windows_and_native_linux_are_separate_sections_with_their_own_headings() {
    use crate::layout_probe::{lay_out, render};
    let page = choosing();
    let size = iced::Size::new(900.0, 720.0);
    let mut laid = lay_out(page.view_with_key_availability(Palette::DARK, true), size);
    let rows = start_rows(&mut laid);
    let texts = texts(&page, size);
    let windows = labeled(&texts, "Windows");
    let native = labeled(&texts, "Native Linux");
    assert_eq!((windows.len(), native.len()), (1, 1), "{texts:?}");
    let (windows, native) = (windows[0], native[0]);

    // Each heading sits above its first row, level with the other, and the
    // Native Linux heading comes after the last Windows row.
    assert!(
        windows.y + windows.height < rows[0].y,
        "{windows:?} {rows:?}"
    );
    assert!(rows[2].y + rows[2].height < native.y, "{native:?} {rows:?}");
    assert!(native.y + native.height < rows[3].y, "{native:?} {rows:?}");
    assert!((windows.x - native.x).abs() < 0.01);

    // Two surfaces: the page's own backdrop shows between the last Windows
    // row and the Native Linux heading, while right under that row (inside
    // its surface) and beside the separators the surface color does.
    let palette = Palette::DARK;
    let mut view = page.view_with_key_availability(palette, true);
    let pixels = render(&mut view, size);
    let width = size.width as u32;
    let x = rows[2].x + rows[2].width - 12.0;
    let below_windows = rows[2].y + rows[2].height + 2.0;
    let between = (rows[2].y + rows[2].height + native.y) / 2.0;
    let above_native_rows = rows[3].y - 2.0;
    assert_eq!(
        pixel_at(&pixels, width, x, below_windows),
        rgba8(palette.surface),
        "the Windows surface runs on a little below its last row"
    );
    assert_ne!(
        pixel_at(&pixels, width, x, between),
        rgba8(palette.surface),
        "the gap between the sections is not surface"
    );
    assert_eq!(
        pixel_at(&pixels, width, x, above_native_rows),
        rgba8(palette.surface),
        "the Native Linux rows have a surface of their own"
    );
    // The CPU renderer doesn't snap a separator to a pixel row, so it may
    // spread over two.
    let divider = rows[0].y + rows[0].height;
    assert!(
        (0..2).any(
            |step| pixel_at(&pixels, width, x, divider + step as f32) != rgba8(palette.surface)
        ),
        "a separator parts rows inside a section"
    );
}
