mod page_tests;

use super::*;
use wineo_core::components::Component;
use wineo_core::validate::{Code, Problem};
use wineo_core::winetricks::Step;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = wineo_core::paths::Paths::from_lookup(move |key| {
        (key == "HOME").then(|| home.as_os_str().to_owned())
    })
    .unwrap();
    let config = paths.environment_config("game");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, "kind = 'native'\n").unwrap();
    let mut app = App::boot(paths).0;
    app.cards = crate::library::load_library(&app.paths).unwrap();
    app.refreshing = false;
    app.trash_refreshing = false;
    (root, app)
}

fn scope() -> Scope {
    Scope::Environment("game".into())
}

fn download() -> Fix {
    Fix::Download {
        component: Component::Dxvk,
        version: Some("missing".into()),
    }
}

fn other_fix() -> Fix {
    Fix::RemoveStateDir {
        environment: "old".into(),
    }
}

fn problem(code: Code, fix: Option<Fix>) -> Problem {
    Problem {
        environment: Some("game".into()),
        template: None,
        stray: None,
        area: code.area(),
        level: code.level(),
        code,
        message: "the CLI's problem message".into(),
        fix,
    }
}

fn report() -> Report {
    Report {
        problems: vec![
            problem(Code::BuildMissing, Some(download())),
            problem(Code::BuildMissing, Some(download())),
            problem(Code::StateDirStray, Some(other_fix())),
        ],
        ..Report::default()
    }
}

fn send(app: &mut App, event: Event) {
    let _ = app.update(Message::Checkup(event));
}

fn loaded(app: &mut App, report: Report) {
    let generation = app.checkups[&scope()].generation;
    send(app, Event::Checked(scope(), generation, Ok(report)));
}

fn open(app: &mut App) {
    send(app, Event::Open(scope()));
}

fn start(app: &mut App) -> u64 {
    open(app);
    loaded(app, report());
    send(app, Event::Fix(scope()));
    app.change_generation
}

fn lines(app: &App) -> Vec<(String, Option<Level>)> {
    let results = &app.checkups[&scope()];
    results.outcome.as_ref().unwrap().lines(results.fixed)
}

fn key(app: &mut App, key: Named, focused: bool, captured: bool) -> Task<Message> {
    app.update(Message::KeyWithFocus(
        Key::Named(key),
        Modifiers::empty(),
        if captured {
            iced::event::Status::Captured
        } else {
            iced::event::Status::Ignored
        },
        focused,
    ))
}

fn add_playable_environment(app: &mut App, name: &str) {
    std::fs::write(app.paths.environment_config(name), "kind = 'native'\n").unwrap();
    app.cards = crate::library::load_library(&app.paths).unwrap();
    let card = app.cards.iter_mut().find(|card| card.name == name).unwrap();
    card.targets = vec!["game".into()];
    card.default_target = Some("game".into());
}

#[test]
fn snapshot_recovery_requires_separate_confirmation_and_cannot_be_canceled() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let fix = Fix::RecoverSnapshot {
        environment: "game".into(),
    };
    loaded(
        &mut app,
        Report {
            problems: vec![problem(Code::SnapshotRestoreInterrupted, Some(fix.clone()))],
            ..Report::default()
        },
    );
    assert!(app.checkups[&scope()].selected().is_empty());
    send(&mut app, Event::Fix(scope()));
    assert!(app.change.is_none());
    send(&mut app, Event::ReviewRecovery(scope(), fix));
    assert!(app.checkups[&scope()].recovery_confirm.is_some());
    assert!(app.change.is_none());
    send(&mut app, Event::ConfirmRecovery(scope()));
    assert!(
        app.checkup_change()
            .is_some_and(|change| change.noncancellable)
    );
    send(&mut app, Event::Cancel);
    assert!(!app.checkup_change().unwrap().token.is_canceled());
    assert!(!app.checkup_force_available());
}

#[test]
fn cancellation_text_omits_zero_and_pluralizes_skipped_fixes() {
    for (skipped, expected) in [
        (0, "Canceled."),
        (1, "Canceled. 1 fix skipped."),
        (2, "Canceled. 2 fixes skipped."),
    ] {
        let outcome = Outcome {
            canceled: true,
            skipped,
            ..Outcome::default()
        };
        assert_eq!(outcome.lines(0).last().unwrap().0, expected);
    }
}

#[test]
fn results_count_problem_rows_for_shared_fixes_and_use_cli_zero_copy() {
    let (_root, mut app) = fixture();
    let generation = start(&mut app);
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                applied: vec![download()],
                ..Outcome::default()
            }),
        ),
    );
    loaded(&mut app, Report::default());
    assert_eq!(lines(&app)[0].0, "Fixed 2 problems.");
    let generation = start(&mut app);
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                failed: vec![(download(), "busy".into())],
                ..Outcome::default()
            }),
        ),
    );
    loaded(&mut app, report());
    assert_eq!(lines(&app)[0].0, "Nothing was fixed.");
}

#[test]
fn menu_opens_panel_results_and_clean_check_has_one_line() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Context("game".into()));
    open(&mut app);
    assert!(app.action_menu.is_none());
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open && app.checkup_visible());
    assert!(app.checkups[&scope()].checking && app.spinning());
    loaded(&mut app, Report::default());
    assert_eq!(
        summary(app.checkups[&scope()].report.as_ref().unwrap()),
        "No problems found."
    );
    assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
    assert!(!app.spinning());
}

#[test]
fn summary_uses_cli_counts_and_messages() {
    let mut report = Report::default();
    report.problems.push(problem(Code::BuildMissing, None));
    report.problems.push(problem(Code::StateDirStray, None));
    assert_eq!(summary(&report), "1 error, 1 warning");
    assert_eq!(report.problems[0].message, "the CLI's problem message");
    report.problems.pop();
    assert_eq!(summary(&report), "1 error");
    report.problems = vec![problem(Code::StateDirStray, None)];
    assert_eq!(summary(&report), "1 warning");
}

#[test]
fn shared_boxes_toggle_together_and_unchecked_fixes_are_not_applied() {
    let (_root, mut app) = fixture();
    open(&mut app);
    loaded(&mut app, report());
    let results = &app.checkups[&scope()];
    assert_eq!(results.selected(), [download(), other_fix()]);
    assert_eq!(
        results.report.as_ref().unwrap().problems[0].fix,
        results.report.as_ref().unwrap().problems[1].fix
    );
    assert_eq!(
        app.checkup_controls(&scope()),
        [
            "checkup-environment-game-fix-0",
            "checkup-environment-game-fix-1",
            "checkup-environment-game-fix-2",
            "checkup-environment-game-apply",
            "checkup-environment-game-check",
            "checkup-environment-game-back"
        ]
    );
    send(&mut app, Event::Toggle(scope(), download(), false));
    assert_eq!(app.checkups[&scope()].selected(), [other_fix()]);
    assert!(!app.checkups[&scope()].checked.contains(&download()));
    let mut applied = Vec::new();
    let outcome = checkup_work::run_fixes(
        &app.checkups[&scope()].selected(),
        &Token::new(),
        |fix, _| {
            applied.push(fix.clone());
            Ok(Vec::new())
        },
    );
    assert_eq!(applied, [other_fix()]);
    assert_eq!(outcome.applied.len(), 1);
    send(&mut app, Event::Toggle(scope(), download(), true));
    let outcome =
        checkup_work::run_fixes(&app.checkups[&scope()].selected(), &Token::new(), |_, _| {
            Ok(Vec::new())
        });
    assert_eq!(outcome.applied, [download(), other_fix()]);
    assert_eq!(outcome.lines(3)[0].0, "Fixed 3 problems.");
}

#[test]
fn failed_checks_show_error_and_check_again_recovers() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let generation = app.checkups[&scope()].generation;
    send(
        &mut app,
        Event::Checked(scope(), generation, Err("storage is unavailable".into())),
    );
    assert_eq!(
        app.checkups[&scope()].error.as_deref(),
        Some("storage is unavailable")
    );
    assert!(app.checkups[&scope()].report.is_none());
    assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
    send(&mut app, Event::Check(scope()));
    assert!(app.checkups[&scope()].checking);
    loaded(&mut app, Report::default());
    assert!(app.checkups[&scope()].error.is_none());
}

#[test]
fn completion_rechecks_and_reloads_both_with_persistent_partial_results() {
    let (_root, mut app) = fixture();
    let generation = start(&mut app);
    let (library, trash, check, downloads) = (
        app.library_generation,
        app.trash_generation,
        app.checkups[&scope()].generation,
        crate::downloads_page::tests::generation(&app),
    );
    let outcome = Outcome {
        applied: vec![download()],
        failed: vec![(other_fix(), "busy".into())],
        warnings: vec!["left something behind".into()],
        canceled: true,
        skipped: 1,
    };
    send(&mut app, Event::Finished(generation, Ok(outcome)));
    assert!(app.change.is_none());
    assert!(app.checkups[&scope()].checking);
    assert!(app.checkups[&scope()].generation > check);
    assert!(app.library_generation > library && app.trash_generation > trash);
    assert!(crate::downloads_page::tests::generation(&app) > downloads);
    loaded(&mut app, Report::default());
    let previous = lines(&app);
    assert_eq!(
        previous
            .iter()
            .map(|(line, _)| line.as_str())
            .collect::<Vec<_>>(),
        [
            "Fixed 2 problems.",
            "error: Remove old's leftover state failed: busy",
            "warning: left something behind",
            "Canceled. 1 fix skipped."
        ]
    );
    let _ = app.update(Message::Refresh);
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert_eq!(lines(&app), previous);
    send(&mut app, Event::Check(scope()));
    assert!(app.checkups[&scope()].checking);
    assert_eq!(lines(&app), previous);
    loaded(&mut app, Report::default());
    assert!(app.checkups[&scope()].outcome.is_none());
}

#[test]
fn unrelated_fix_does_not_reload_downloads_after_switching_pages() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let fix = Fix::RemoveStatsLog {
        environment: "old".into(),
    };
    loaded(
        &mut app,
        Report {
            problems: vec![problem(Code::StatsStray, Some(fix.clone()))],
            ..Report::default()
        },
    );
    assert_eq!(
        app.checkups[&scope()].selected().as_slice(),
        std::slice::from_ref(&fix)
    );
    send(&mut app, Event::Fix(scope()));
    let _ = app.update(Message::Section(crate::sidebar::Section::Downloads));
    let generation = app.change_generation;
    let downloads = crate::downloads_page::tests::generation(&app);

    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                applied: vec![fix],
                ..Outcome::default()
            }),
        ),
    );

    assert_eq!(crate::downloads_page::tests::generation(&app), downloads);
}

#[test]
fn another_change_disables_fixes_but_checking_still_works() {
    let (_root, mut app) = fixture();
    app.change = Some(RunningChange::Restore {
        id: "entry".into(),
        name: "other".into(),
    });
    open(&mut app);
    assert!(app.checkups[&scope()].checking);
    loaded(&mut app, report());
    assert!(!app.checkup_can_fix(&app.checkups[&scope()]));
    assert!(
        !app.checkup_controls(&scope())
            .contains(&"checkup-environment-game-apply".into())
    );
    assert_eq!(
        app.change.as_ref().unwrap().reason(),
        "Busy: restoring other"
    );
    let generation = app.change_generation;
    send(&mut app, Event::Fix(scope()));
    assert_eq!(app.change_generation, generation);
    assert!(matches!(app.change, Some(RunningChange::Restore { .. })));
    send(&mut app, Event::Check(scope()));
    assert!(app.checkups[&scope()].checking);
}

#[test]
fn section_switch_and_panel_close_keep_the_batch_but_reopen_details() {
    let (_root, mut app) = fixture();
    let generation = start(&mut app);
    assert_eq!(app.change.as_ref().unwrap().reason(), "Busy: fixing game");
    let token = app.checkup_change().unwrap().token.clone();
    let _ = app.update(Message::Section(crate::sidebar::Section::Trash));
    let _ = app.update(Message::ClosePanel);
    assert!(!token.is_canceled());
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let _ = app.update(Message::Select("game".into()));
    assert!(app.panel_open && !app.checkup_visible());
    assert_eq!(app.checkup_change().unwrap().status, "Fixing...");
    let check = app.checkups[&scope()].generation;
    open(&mut app);
    assert!(app.checkup_visible() && app.checkups[&scope()].checking);
    assert!(app.checkups[&scope()].generation > check);
    assert!(!token.is_canceled());
    let _ = app.update(Message::ClosePanel);
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                applied: vec![download()],
                ..Outcome::default()
            }),
        ),
    );
    loaded(&mut app, Report::default());
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let _ = app.update(Message::Select("game".into()));
    assert!(app.panel_open && !app.checkup_visible());
    assert_eq!(lines(&app)[0].0, "Fixed 2 problems.");
    open(&mut app);
    assert!(app.checkup_visible() && app.checkups[&scope()].checking);
    assert_eq!(lines(&app)[0].0, "Fixed 2 problems.");
    loaded(&mut app, Report::default());
    assert!(app.checkups[&scope()].outcome.is_none());
    let _ = key(&mut app, Named::Escape, false, false);
    assert!(app.panel_open && !app.checkup_visible());
}

#[test]
fn selection_changes_leave_results_for_mouse_and_keyboard() {
    for keyboard in [false, true] {
        let (_root, mut app) = fixture();
        add_playable_environment(&mut app, "other");
        open(&mut app);
        loaded(&mut app, report());
        if keyboard {
            let _ = key(&mut app, Named::ArrowRight, false, false);
        } else {
            let _ = app.update(Message::Select("other".into()));
        }
        assert_eq!(app.selected.as_deref(), Some("other"));
        assert!(!app.checkups[&scope()].showing);
        let _ = app.update(Message::Select("game".into()));
        assert!(app.panel_open && !app.checkup_visible());
        assert!(app.checkups[&scope()].report.is_some());
    }
}

#[test]
fn results_keys_respect_search_focus() {
    let (_root, mut app) = fixture();
    open(&mut app);
    loaded(&mut app, report());
    app.search_focused = false;
    assert_eq!(key(&mut app, Named::Tab, true, true).units(), 0);
    assert!(app.search_focused && app.checkup_visible());
    assert!(key(&mut app, Named::Tab, false, false).units() > 0);
    assert!(!app.search_focused && app.checkup_visible());
    let _ = key(&mut app, Named::Escape, false, false);
    assert!(app.panel_open && !app.checkup_visible());
    open(&mut app);
    // Search keeps the existing panel Escape behavior instead of results Back.
    let _ = key(&mut app, Named::Escape, true, false);
    assert!(!app.panel_open);
}

#[test]
fn unfocused_enter_does_not_play_from_results_and_log_keeps_its_behavior() {
    let (_root, mut app) = fixture();
    add_playable_environment(&mut app, "game");
    open(&mut app);
    loaded(&mut app, report());
    let _ = key(&mut app, Named::Enter, false, false);
    assert!(app.operation.is_none() && app.checkup_visible());
    app.log_view = true;
    let _ = key(&mut app, Named::Enter, false, false);
    assert!(app.operation.as_ref().is_some_and(|operation| {
        operation.name == "game" && operation.kind == crate::OperationKind::Start
    }));
}

#[test]
fn modal_guard_blocks_fix_and_check_events() {
    for removal in [false, true] {
        let (_root, mut app) = fixture();
        open(&mut app);
        loaded(&mut app, report());
        if removal {
            let _ = app.update(Message::Remove("game".into()));
            assert!(app.removal.is_some());
        } else {
            app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
        }
        let generation = app.checkups[&scope()].generation;
        send(&mut app, Event::Check(scope()));
        send(&mut app, Event::Fix(scope()));
        assert_eq!(app.checkups[&scope()].generation, generation);
        assert!(!app.checkups[&scope()].checking);
        assert!(app.change.is_none());
    }
}

#[test]
fn removed_and_restored_environment_rejects_the_old_checks_reply() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let old_check = app.checkups[&scope()].generation;
    let _ = app.update(Message::Remove("game".into()));
    let _ = app.update(Message::ConfirmRemove);
    let entry = wineo_core::trash::remove(&app.paths, "game", false).unwrap();
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Ok(entry.clone()),
    ));
    assert!(!app.checkups.contains_key(&scope()));
    let _ = app.update(Message::Undo);
    wineo_core::trash::restore(&app.paths, &entry.id).unwrap();
    let _ = app.update(Message::UndoFinished(app.change_generation, Ok(Vec::new())));
    let _ = app.update(Message::Loaded(Ok(crate::library::load_library(
        &app.paths,
    )
    .unwrap())));
    open(&mut app);
    assert!(app.checkups[&scope()].generation > old_check);
    send(&mut app, Event::Checked(scope(), old_check, Ok(report())));
    assert!(app.checkups[&scope()].checking);
    assert!(app.checkups[&scope()].report.is_none());
    loaded(&mut app, Report::default());
    assert!(!app.checkups[&scope()].checking);
    assert!(
        app.checkups[&scope()]
            .report
            .as_ref()
            .unwrap()
            .problems
            .is_empty()
    );
}

#[test]
fn play_on_another_environment_keeps_fix_status_and_log_sources_independent() {
    let (_root, mut app) = fixture();
    add_playable_environment(&mut app, "other");
    let generation = start(&mut app);
    let fix_done = Token::new();
    send(
        &mut app,
        Event::InstallStarted(generation, "game".into(), fix_done.clone()),
    );
    send(
        &mut app,
        Event::Progress(generation, "Fix installation".into()),
    );
    let _ = app.update(Message::Select("other".into()));
    let _ = app.update(Message::Play);
    let play_done = app
        .operation
        .as_ref()
        .unwrap()
        .install_done
        .clone()
        .unwrap();
    let _ = app.update(Message::InstallPhase("other".into(), true));
    let _ = app.update(Message::LaunchProgress(
        "other".into(),
        "Play installation".into(),
    ));
    assert_eq!(app.operation.as_ref().unwrap().status, "Play installation");
    assert_eq!(app.checkup_change().unwrap().status, "Fix installation");
    for (name, done) in [("game", &fix_done), ("other", &play_done)] {
        let card = app.cards.iter().find(|card| card.name == name).unwrap();
        let Some(crate::worker::LogSource::Install(source_done)) = app.log_source(card) else {
            panic!("missing {name} install log");
        };
        assert!(!source_done.is_canceled() && !done.is_canceled());
    }
    let _ = app.update(Message::OpenLog);
    assert!(app.log_view && app.log_install);
    let play_log = app.log_generation;
    let _ = app.update(Message::LogChunk(play_log, "play log\n".into()));
    send(
        &mut app,
        Event::Progress(generation, "Fix still running".into()),
    );
    assert_eq!(app.operation.as_ref().unwrap().status, "Play installation");
    assert_eq!(app.log_buffer.display(), "play log\n");
    let _ = app.update(Message::Select("game".into()));
    assert!(!app.log_view && app.log_generation > play_log);
    let _ = app.update(Message::OpenLog);
    assert!(app.log_view && app.log_install);
    let fix_log = app.log_generation;
    let _ = app.update(Message::LogChunk(play_log, "stale play log\n".into()));
    assert!(app.log_buffer.display().is_empty());
    let _ = app.update(Message::LogChunk(fix_log, "fix log\n".into()));
    assert_eq!(app.log_buffer.display(), "fix log\n");
    send(&mut app, Event::InstallEnded(generation));
    assert!(fix_done.is_canceled() && !play_done.is_canceled());
    let game = app.cards.iter().find(|card| card.name == "game").unwrap();
    assert!(app.log_source(game).is_none());
    let other = app.cards.iter().find(|card| card.name == "other").unwrap();
    assert!(matches!(
        app.log_source(other),
        Some(crate::worker::LogSource::Install(_))
    ));
    let _ = app.update(Message::Select("other".into()));
    let _ = app.update(Message::OpenLog);
    assert!(app.log_view && app.log_install);
    let _ = app.update(Message::LogChunk(fix_log, "stale fix log\n".into()));
    assert!(app.log_buffer.display().is_empty());
    assert_eq!(app.checkup_change().unwrap().status, "Fix still running");
    let _ = app.update(Message::LaunchProgress(
        "other".into(),
        "Play starting".into(),
    ));
    assert_eq!(app.checkup_change().unwrap().status, "Fix still running");
    assert_eq!(app.operation.as_ref().unwrap().status, "Play starting");
}

#[test]
fn winetricks_progress_and_log_started_use_existing_live_log_source() {
    let (_root, mut app) = fixture();
    let generation = start(&mut app);
    assert!(app.checkup_log_source("game").is_none());
    send(
        &mut app,
        Event::Winetricks(
            generation,
            Arc::new(WinetricksEvent::Progress(Step {
                verb: "gdiplus".into(),
                position: 2,
                total: 3,
            })),
        ),
    );
    assert_eq!(
        app.checkup_change().unwrap().status,
        "Installing gdiplus (2 of 3)..."
    );
    let done = Token::new();
    send(
        &mut app,
        Event::InstallStarted(generation, "game".into(), done.clone()),
    );
    assert!(matches!(
        app.log_source(&app.cards[0]),
        Some(crate::worker::LogSource::Install(_))
    ));
    assert!(
        app.checkup_controls(&scope())
            .contains(&"checkup-environment-game-log".into())
    );
    let _ = app.update(Message::OpenLog);
    assert!(app.log_view && app.log_install);
    let _ = app.update(Message::CloseLog);
    assert!(app.checkup_visible());
    send(&mut app, Event::InstallEnded(generation));
    assert!(done.is_canceled());
    assert!(app.checkup_log_source("game").is_none());
    assert!(
        !app.checkup_controls(&scope())
            .contains(&"checkup-environment-game-log".into())
    );
}

#[test]
fn cancel_and_close_cancels_and_waits_with_force_stop_for_winetricks() {
    let (_root, mut app) = fixture();
    let generation = start(&mut app);
    let token = app.checkup_change().unwrap().token.clone();
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.close_prompt && !token.is_canceled());
    let _ = app.update(Message::KeepWorking);
    assert!(!app.close_prompt);
    send(
        &mut app,
        Event::InstallStarted(generation, "game".into(), Token::new()),
    );
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && app.foreground_busy() && token.is_canceled());
    assert!(app.checkup_force_available());
    let _ = app.update(Message::ForceCloseWork);
    assert!(token.is_forced());
    assert!(app.change.is_some());
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                canceled: true,
                skipped: 1,
                ..Outcome::default()
            }),
        ),
    );
    assert!(!app.foreground_busy());
    assert!(app.checkups[&scope()].checking);
}

#[test]
fn cancel_then_force_stop_use_one_batch_token() {
    let (_root, mut app) = fixture();
    start(&mut app);
    let token = app.checkup_change().unwrap().token.clone();
    send(&mut app, Event::Cancel);
    assert!(token.is_canceled() && !token.is_forced());
    send(&mut app, Event::Cancel);
    assert!(token.is_forced());
}

#[test]
fn stale_check_and_fix_results_never_win() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let old_check = app.checkups[&scope()].generation;
    send(&mut app, Event::Check(scope()));
    send(&mut app, Event::Checked(scope(), old_check, Ok(report())));
    assert!(app.checkups[&scope()].checking && app.checkups[&scope()].report.is_none());
    loaded(&mut app, report());
    send(&mut app, Event::Fix(scope()));
    let generation = app.change_generation;
    send(
        &mut app,
        Event::Progress(generation.wrapping_sub(1), "stale".into()),
    );
    send(
        &mut app,
        Event::Finished(generation.wrapping_sub(1), Ok(Outcome::default())),
    );
    assert!(app.change.is_some());
    assert_ne!(app.checkup_change().unwrap().status, "stale");
    send(&mut app, Event::Check(scope()));
    let concurrent_check = app.checkups[&scope()].generation;
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                applied: vec![download()],
                ..Outcome::default()
            }),
        ),
    );
    send(
        &mut app,
        Event::Checked(scope(), concurrent_check, Ok(report())),
    );
    assert!(app.checkups[&scope()].checking);
    loaded(&mut app, Report::default());
    send(
        &mut app,
        Event::Finished(generation, Ok(Outcome::default())),
    );
    send(
        &mut app,
        Event::InstallStarted(generation, "game".into(), Token::new()),
    );
    assert!(app.checkup_change().is_none());
    assert_eq!(
        app.checkups[&scope()].outcome.as_ref().unwrap().applied,
        [download()]
    );
    assert!(
        app.checkups[&scope()]
            .report
            .as_ref()
            .unwrap()
            .problems
            .is_empty()
    );
}
