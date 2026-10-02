use super::*;
use crate::sidebar::Section;
use crate::trash_page::{Event as TrashEvent, Selection};
use std::fs;
use wineo_core::{trash, validate};

fn accept(app: &mut App, scope: Scope, report: Report) {
    let generation = app.checkups[&scope].generation;
    send(app, Event::Checked(scope, generation, Ok(report)));
}

fn page(app: &mut App) {
    let _ = app.update(Message::Section(Section::Checkup));
}

fn start_page(app: &mut App) -> u64 {
    page(app);
    accept(app, Scope::Global, report());
    send(app, Event::Fix(Scope::Global));
    app.change_generation
}

#[test]
fn page_checks_everything_on_open_and_never_on_startup_or_tick() {
    let (_root, mut app) = fixture();
    assert!(app.checkups.is_empty());
    fs::write(app.paths.global_config(), "broken = [").unwrap();
    fs::create_dir_all(app.paths.templates_dir()).unwrap();
    fs::write(app.paths.templates_dir().join("bad.toml"), "broken = [").unwrap();
    let _ = app.update(Message::Refresh);
    assert!(app.checkups.is_empty());
    page(&mut app);
    assert!(app.selected.is_none() && !app.panel_open);
    assert!(app.checkups[&Scope::Global].checking && app.spinning());
    let report = checkup_work::check_report(&app.paths, &Scope::Global).unwrap();
    assert_eq!(report.environments, ["game"]);
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == Code::GlobalConfigInvalid)
    );
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.template.as_deref() == Some("bad"))
    );
    accept(&mut app, Scope::Global, report);
    let generation = app.checkups[&Scope::Global].generation;
    let _ = app.update(Message::Refresh);
    assert_eq!(app.checkups[&Scope::Global].generation, generation);
    assert!(!app.checkups[&Scope::Global].checking);
    send(&mut app, Event::Check(Scope::Global));
    assert!(app.checkups[&Scope::Global].generation > generation);
}

#[test]
fn page_groups_cli_files_environments_and_leftovers_without_healthy_rows() {
    let (_root, mut app) = fixture();
    let mut global = problem(Code::GlobalConfigInvalid, None);
    global.environment = None;
    let mut template = problem(Code::TemplateInvalid, None);
    template.environment = None;
    template.template = Some("mine".into());
    let mut leftover = problem(Code::StateDirStray, Some(other_fix()));
    leftover.environment = None;
    leftover.stray = Some("old".into());
    let report = Report {
        environments: vec!["game".into(), "healthy".into()],
        templates: vec!["mine".into(), "healthy-template".into()],
        problems: vec![
            global,
            template,
            problem(Code::BuildMissing, Some(download())),
            leftover,
        ],
    };
    let grouped = groups(&report, &Scope::Global);
    assert_eq!(
        grouped
            .iter()
            .map(|(group, _)| group.clone())
            .collect::<Vec<_>>(),
        [
            Group::Global,
            Group::Template("mine".into()),
            Group::Environment("game".into()),
            Group::Leftover("old".into())
        ]
    );
    assert_eq!(
        grouped[0].0.heading(&app.paths),
        "~/.config/wineo/config.toml"
    );
    assert_eq!(
        grouped[1].0.heading(&app.paths),
        "~/.config/wineo/templates/mine.toml"
    );
    assert_eq!(grouped[2].0.heading(&app.paths), "game");
    assert_eq!(grouped[3].0.heading(&app.paths), "old");
    assert!(grouped[3].0.environment().is_none());
    page(&mut app);
    accept(&mut app, Scope::Global, report);
    assert_eq!(
        app.checkup_controls(&Scope::Global),
        [
            "checkup-global-heading-2",
            "checkup-global-fix-2",
            "checkup-global-fix-3",
            "checkup-global-apply",
            "checkup-global-check",
        ]
    );
    let _ = app.checkup_page();
}

#[test]
fn heading_opens_library_details_even_when_that_panel_already_shows_results() {
    let (_root, mut app) = fixture();
    open(&mut app);
    loaded(&mut app, report());
    page(&mut app);
    send(&mut app, Event::Environment("game".into()));
    assert_eq!(app.section, Section::Library);
    assert_eq!(app.selected.as_deref(), Some("game"));
    assert!(app.panel_open && !app.checkup_visible());
    assert!(!app.log_view);
    page(&mut app);
    send(&mut app, Event::Environment("missing".into()));
    assert_eq!(app.section, Section::Checkup);
}

#[test]
fn shared_fix_has_a_box_in_each_environment_and_runs_once() {
    let (_root, mut app) = fixture();
    add_playable_environment(&mut app, "other");
    let mut other = problem(Code::BuildMissing, Some(download()));
    other.environment = Some("other".into());
    let report = Report {
        problems: vec![problem(Code::BuildMissing, Some(download())), other],
        ..Report::default()
    };
    let grouped = groups(&report, &Scope::Global);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].1[0].1.fix, grouped[1].1[0].1.fix);
    page(&mut app);
    accept(&mut app, Scope::Global, report);
    let controls = app.checkup_controls(&Scope::Global);
    assert!(controls.contains(&Scope::Global.id("fix-0")));
    assert!(controls.contains(&Scope::Global.id("fix-1")));
    send(&mut app, Event::Toggle(Scope::Global, download(), false));
    assert!(app.checkups[&Scope::Global].selected().is_empty());
    send(&mut app, Event::Toggle(Scope::Global, download(), true));
    assert_eq!(app.checkups[&Scope::Global].selected(), [download()]);
    send(&mut app, Event::Fix(Scope::Global));
    let mut calls = Vec::new();
    let outcome = checkup_work::run_fixes(
        &app.checkups[&Scope::Global].selected(),
        &Token::new(),
        |fix, _| {
            calls.push(fix.clone());
            Ok(Vec::new())
        },
    );
    assert_eq!(calls, [download()]);
    let generation = app.change_generation;
    send(&mut app, Event::Finished(generation, Ok(outcome)));
    assert_eq!(app.checkups[&Scope::Global].fixed, 2);
}

#[test]
fn actual_interrupted_trash_entry_is_offered_and_recovered_by_page_batch() {
    let Some((_root, _config, mut app)) = crate::trash_page::tests::pending_fixture() else {
        return;
    };
    let config = app.paths.environment_config("interrupted");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
    let entry = crate::trash_page::tests::interrupted_remove(&app.paths, "interrupted");
    page(&mut app);
    let report = checkup_work::check_report(&app.paths, &Scope::Global).unwrap();
    let fix = Fix::RecoverTrash {
        id: entry.id.clone(),
    };
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == Code::TrashInterrupted && p.fix.as_ref() == Some(&fix))
    );
    accept(&mut app, Scope::Global, report);
    assert_eq!(
        app.checkups[&Scope::Global].selected(),
        std::slice::from_ref(&fix)
    );
    send(&mut app, Event::Fix(Scope::Global));
    let (library, trash) = (app.library_generation, app.trash_generation);
    let outcome = checkup_work::run_fixes(&[fix], &Token::new(), |fix, token| {
        validate::apply(
            &app.paths,
            fix,
            token,
            wineo_core::winetricks::EnsureOutput::Stderr,
            |_| {},
            |_, _| anyhow::bail!("unexpected download"),
        )
    });
    assert_eq!(outcome.applied.len(), 1);
    assert!(outcome.failed.is_empty());
    let generation = app.change_generation;
    send(&mut app, Event::Finished(generation, Ok(outcome)));
    assert!(app.library_generation > library && app.trash_generation > trash);
    assert!(trash::pending_entries(&app.paths).unwrap().is_empty());
    let report = checkup_work::check_report(&app.paths, &Scope::Global).unwrap();
    assert!(
        !report
            .problems
            .iter()
            .any(|p| p.code == Code::TrashInterrupted)
    );
    accept(&mut app, Scope::Global, report);
    assert!(!app.checkups[&Scope::Global].checking);
    let _ = app.checkup_page();
}

#[test]
fn install_log_follows_the_batch_environment_with_another_or_no_selected_card() {
    for scope in [scope(), Scope::Global] {
        for selected in [None, Some("other".to_owned())] {
            let (_root, mut app) = fixture();
            add_playable_environment(&mut app, "other");
            let generation = if scope == Scope::Global {
                start_page(&mut app)
            } else {
                start(&mut app)
            };
            app.selected = selected.clone();
            let done = Token::new();
            send(
                &mut app,
                Event::InstallStarted(generation, "game".into(), done.clone()),
            );
            let (name, source) = app.checkup_install_log(&scope).unwrap();
            assert_eq!(name, "game");
            assert!(matches!(source, crate::worker::LogSource::Install(_)));
            send(&mut app, Event::OpenInstallLog(scope.clone()));
            assert!(app.log_view && app.log_install);
            assert_eq!(app.checkup_log_scope.as_ref(), Some(&scope));
            assert_eq!(app.selected, selected);
            let _ = app.update(Message::LogChunk(
                app.log_generation,
                "install log\n".into(),
            ));
            assert_eq!(app.log_buffer.display(), "install log\n");
            if scope == Scope::Global {
                let _ = app.checkup_page();
                let _ = app.update(Message::Section(Section::Library));
                assert!(!app.panel_log_visible());
            }
            send(&mut app, Event::InstallEnded(generation));
            assert!(done.is_canceled() && app.checkup_install_log(&scope).is_none());
        }
    }
}

#[test]
fn hidden_global_install_log_does_not_block_library_delete_or_bypass_panel_results() {
    for delete in [true, false] {
        let (_root, mut app) = fixture();
        add_playable_environment(&mut app, "game");
        open(&mut app);
        loaded(&mut app, report());
        let generation = start_page(&mut app);
        send(
            &mut app,
            Event::InstallStarted(generation, "game".into(), Token::new()),
        );
        send(&mut app, Event::OpenInstallLog(Scope::Global));
        send(
            &mut app,
            Event::Finished(generation, Ok(Outcome::default())),
        );
        let _ = app.update(Message::Section(Section::Library));
        assert!(app.log_view && !app.panel_log_visible());
        if delete {
            app.close_checkup();
            let _ = crate::removal::delete_key(&mut app, false, false);
            assert_eq!(app.removal.as_ref().unwrap().name, "game");
        } else {
            loaded(&mut app, Report::default());
            assert!(app.checkup_visible());
            let _ = key(&mut app, Named::Enter, false, false);
            assert!(app.operation.is_none());
        }
    }
}

#[test]
fn global_install_log_errors_stay_with_the_log_and_ignore_stale_replies() {
    let (_root, mut app) = fixture();
    add_playable_environment(&mut app, "other");
    let generation = start_page(&mut app);
    send(
        &mut app,
        Event::InstallStarted(generation, "game".into(), Token::new()),
    );
    send(&mut app, Event::OpenInstallLog(Scope::Global));
    let log_generation = app.log_generation;
    app.selected = Some("other".into());
    app.panel_open = true;
    let _ = app.update(Message::Section(Section::Library));
    let _ = app.update(Message::LogFinished(
        log_generation,
        Err("log failed".into()),
    ));
    assert_eq!(app.log_error.as_deref(), Some("log failed"));
    assert!(app.panel_note.is_none() && !app.panel_log_visible());
    page(&mut app);
    assert_eq!(app.checkup_log_scope, Some(Scope::Global));
    let _ = app.checkup_page();
    let _ = app.update(Message::LogFinished(
        log_generation.wrapping_sub(1),
        Err("stale".into()),
    ));
    assert_eq!(app.log_error.as_deref(), Some("log failed"));
    app.close_log();
    assert!(app.log_error.is_none());
    let _ = app.update(Message::LogFinished(log_generation, Err("closed".into())));
    assert!(app.log_error.is_none() && app.panel_note.is_none());
}

#[test]
fn finished_batches_refresh_other_showing_scopes_without_clearing_outcomes() {
    for batch_scope in [scope(), Scope::Global] {
        let (_root, mut app) = fixture();
        add_playable_environment(&mut app, "other");
        let other = Scope::Environment("other".into());
        let hidden = Scope::Environment("hidden".into());
        open(&mut app);
        loaded(&mut app, report());
        page(&mut app);
        accept(&mut app, Scope::Global, report());
        app.checkups.insert(
            other.clone(),
            Results {
                showing: true,
                report: Some(report()),
                ..Results::default()
            },
        );
        app.checkups.insert(
            hidden.clone(),
            Results {
                report: Some(report()),
                ..Results::default()
            },
        );
        for scope in [scope(), Scope::Global, other.clone()] {
            app.checkups.get_mut(&scope).unwrap().outcome = Some(Outcome {
                warnings: vec!["previous outcome".into()],
                ..Outcome::default()
            });
        }
        send(&mut app, Event::Fix(batch_scope.clone()));
        let _ = app.update(Message::Section(Section::Trash));
        let generations =
            [scope(), Scope::Global, other.clone()].map(|scope| app.checkups[&scope].generation);
        let generation = app.change_generation;
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
        for (scope, old_generation) in [scope(), Scope::Global, other].into_iter().zip(generations)
        {
            let results = &app.checkups[&scope];
            assert!(results.checking && results.generation > old_generation);
            assert!(results.outcome.is_some());
            assert!(!app.checkup_can_fix(results));
            send(
                &mut app,
                Event::Checked(scope.clone(), old_generation, Ok(report())),
            );
            assert!(app.checkups[&scope].checking);
            accept(&mut app, scope.clone(), Report::default());
            let results = &app.checkups[&scope];
            assert!(results.selected().is_empty());
            assert!(!app.checkup_controls(&scope).contains(&scope.id("fix-0")));
            let outcome = results.outcome.as_ref().unwrap();
            if scope == batch_scope {
                assert_eq!(outcome.applied, [download()]);
            } else {
                assert_eq!(outcome.warnings, ["previous outcome"]);
            }
        }
        assert!(!app.checkups[&hidden].checking);
        assert_eq!(app.checkups[&hidden].generation, 0);
    }
}

#[test]
fn visibility_keyboard_and_ids_are_scoped_and_page_escape_keeps_results() {
    let (_root, mut app) = fixture();
    open(&mut app);
    loaded(&mut app, report());
    let panel_controls = app.checkup_controls(&scope());
    page(&mut app);
    accept(&mut app, Scope::Global, report());
    let page_controls = app.checkup_controls(&Scope::Global);
    assert!(panel_controls.iter().all(|id| !page_controls.contains(id)));
    app.selected = None;
    app.panel_open = false;
    assert_eq!(app.checkup_key_scope(), Some(Scope::Global));
    assert!(key(&mut app, Named::Tab, false, false).units() > 0);
    let generation = app.checkups[&Scope::Global].generation;
    let _ = key(&mut app, Named::Escape, false, false);
    assert_eq!(app.section, Section::Checkup);
    assert_eq!(app.checkups[&Scope::Global].generation, generation);
    app.action_menu = Some("game".into());
    assert!(app.checkup_key_scope().is_none());
    app.action_menu = None;
    let _ = app.update(Message::Section(Section::Trash));
    assert!(!app.checkup_visible() && app.checkup_key_scope().is_none());
    let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
    assert!(app.settings.is_some() && app.checkup_key_scope().is_none());
}

#[test]
fn tab_still_reaches_undo_with_hidden_panel_results_or_settings() {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::{Focusable, Outcome as FocusOutcome};

    #[derive(Default)]
    struct Focus(bool);
    impl Focusable for Focus {
        fn is_focused(&self) -> bool {
            self.0
        }
        fn focus(&mut self) {
            self.0 = true;
        }
        fn unfocus(&mut self) {
            self.0 = false;
        }
    }

    let (_root, mut app) = fixture();
    add_playable_environment(&mut app, "removed");
    let _ = app.update(Message::Remove("removed".into()));
    let _ = app.update(Message::ConfirmRemove);
    let entry = trash::remove(&app.paths, "removed", false).unwrap();
    let _ = app.update(Message::RemoveFinished(app.change_generation, Ok(entry)));
    assert!(app.toast.is_some());
    open(&mut app);
    loaded(&mut app, report());
    for section in [Section::Trash, Section::Checkup] {
        let _ = app.update(Message::Section(section));
        assert!(app.checkups[&scope()].showing);
        if section == Section::Trash {
            assert!(app.checkup_key_scope().is_none());
            assert!(
                crate::removal::key(
                    &mut app,
                    &Key::Named(Named::Tab),
                    Modifiers::empty(),
                    iced::event::Status::Ignored
                )
                .is_some()
            );
        } else {
            accept(&mut app, Scope::Global, Report::default());
            assert!(
                app.checkup_controls(&Scope::Global)
                    .contains(&focus_button::UNDO_ID.into())
            );
        }
        let _ = focus_button::take_cycles();
        let tab = || {
            Message::Key(
                Key::Named(Named::Tab),
                Modifiers::empty(),
                iced::event::Status::Ignored,
            )
        };
        assert!(app.update(tab()).units() > 0);
        if section == Section::Checkup {
            assert!(focus_button::take_cycles().is_empty());
            let _ = app.update(Message::KeyWithFocus(
                Key::Named(Named::Tab),
                Modifiers::empty(),
                iced::event::Status::Ignored,
                false,
            ));
        }
        let mut cycles = focus_button::take_cycles();
        assert_eq!(cycles.len(), 1);
        let cycle = cycles.pop().unwrap();
        let FocusOutcome::Chain(mut focus) = cycle.finish() else {
            panic!("missing Tab focus operation")
        };
        let mut undo = Focus::default();
        let mut check = Focus::default();
        focus.focusable(
            Some(&iced::widget::Id::new(focus_button::UNDO_ID)),
            iced::Rectangle::default(),
            &mut undo,
        );
        focus.focusable(
            Some(&iced::widget::Id::new("checkup-global-check")),
            iced::Rectangle::default(),
            &mut check,
        );
        if section == Section::Trash {
            assert!(undo.is_focused() && !check.is_focused());
        } else {
            assert!(check.is_focused() && !undo.is_focused());
            let _ = app.update(Message::KeyWithFocus(
                Key::Named(Named::Tab),
                Modifiers::empty(),
                iced::event::Status::Ignored,
                false,
            ));
            let mut cycle = focus_button::take_cycles().pop().unwrap();
            cycle.focusable(
                Some(&iced::widget::Id::new("checkup-global-check")),
                iced::Rectangle::default(),
                &mut check,
            );
            let FocusOutcome::Chain(mut focus) = cycle.finish() else {
                panic!("missing Undo focus operation")
            };
            focus.focusable(
                Some(&iced::widget::Id::new(focus_button::UNDO_ID)),
                iced::Rectangle::default(),
                &mut undo,
            );
            assert!(undo.is_focused());
        }
    }
    let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
    assert!(app.settings.is_some() && app.checkup_key_scope().is_none());
    assert!(
        app.update(Message::Key(
            Key::Named(Named::Tab),
            Modifiers::empty(),
            iced::event::Status::Ignored
        ))
        .units()
            > 0
    );
    assert!(focus_button::take_cycles().is_empty());
}

#[test]
fn page_batch_survives_navigation_and_rechecks_reloads_and_retains_results() {
    let (_root, mut app) = fixture();
    let generation = start_page(&mut app);
    assert_eq!(
        app.change.as_ref().unwrap().reason(),
        "Busy: fixing problems"
    );
    let token = app.checkup_change().unwrap().token.clone();
    let check = app.checkups[&Scope::Global].generation;
    for section in [Section::Library, Section::Trash, Section::Checkup] {
        let _ = app.update(Message::Section(section));
        assert!(app.change.is_some() && !token.is_canceled());
    }
    assert_eq!(app.checkups[&Scope::Global].generation, check);
    let _ = app.update(Message::Section(Section::Library));
    let (library, trash) = (app.library_generation, app.trash_generation);
    send(
        &mut app,
        Event::Finished(
            generation,
            Ok(Outcome {
                applied: vec![download()],
                failed: vec![(other_fix(), "busy".into())],
                ..Outcome::default()
            }),
        ),
    );
    assert!(app.checkups[&Scope::Global].checking);
    assert!(app.library_generation > library && app.trash_generation > trash);
    accept(&mut app, Scope::Global, Report::default());
    let _ = app.update(Message::Section(Section::Library));
    page(&mut app);
    assert!(app.checkups[&Scope::Global].checking);
    assert_eq!(
        app.checkups[&Scope::Global]
            .outcome
            .as_ref()
            .unwrap()
            .applied,
        [download()]
    );
    accept(&mut app, Scope::Global, Report::default());
    let results = &app.checkups[&Scope::Global];
    assert_eq!(results.outcome.as_ref().unwrap().applied, [download()]);
    assert_eq!(
        results.outcome.as_ref().unwrap().lines(results.fixed)[0].0,
        "Fixed 2 problems."
    );
    assert!(
        results.outcome.as_ref().unwrap().lines(results.fixed)[1]
            .0
            .ends_with("failed: busy")
    );
    send(&mut app, Event::Check(Scope::Global));
    assert!(app.checkups[&Scope::Global].outcome.is_some());
    accept(&mut app, Scope::Global, Report::default());
    assert!(app.checkups[&Scope::Global].outcome.is_none());
}

#[test]
fn page_close_prompt_cancels_waits_and_can_force_stop_winetricks() {
    let (_root, mut app) = fixture();
    let generation = start_page(&mut app);
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
    let _ = app.update(Message::ForceCloseWork);
    assert!(token.is_forced() && app.change.is_some());
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
}

#[test]
fn global_stale_checks_and_batch_replies_are_dropped() {
    let (_root, mut app) = fixture();
    page(&mut app);
    let old = app.checkups[&Scope::Global].generation;
    send(&mut app, Event::Check(Scope::Global));
    send(&mut app, Event::Checked(Scope::Global, old, Ok(report())));
    assert!(app.checkups[&Scope::Global].checking && app.checkups[&Scope::Global].report.is_none());
    accept(&mut app, Scope::Global, report());
    send(&mut app, Event::Fix(Scope::Global));
    let generation = app.change_generation;
    send(
        &mut app,
        Event::Progress(generation.wrapping_sub(1), "stale".into()),
    );
    send(
        &mut app,
        Event::Finished(generation.wrapping_sub(1), Ok(Outcome::default())),
    );
    assert!(app.change.is_some() && app.checkup_change().unwrap().status != "stale");
    send(&mut app, Event::Check(Scope::Global));
    let old = app.checkups[&Scope::Global].generation;
    send(
        &mut app,
        Event::Finished(generation, Ok(Outcome::default())),
    );
    send(&mut app, Event::Checked(Scope::Global, old, Ok(report())));
    assert!(app.checkups[&Scope::Global].checking);
    accept(&mut app, Scope::Global, Report::default());
    send(&mut app, Event::Finished(generation, Err("stale".into())));
    assert!(
        app.checkups[&Scope::Global]
            .outcome
            .as_ref()
            .unwrap()
            .warnings
            .is_empty()
    );
}

#[test]
fn panel_page_remove_and_trash_changes_are_mutually_exclusive() {
    for owner in 0..4 {
        let (_root, mut app) = fixture();
        open(&mut app);
        loaded(&mut app, report());
        page(&mut app);
        accept(&mut app, Scope::Global, report());
        match owner {
            0 => send(&mut app, Event::Fix(scope())),
            1 => send(&mut app, Event::Fix(Scope::Global)),
            2 => {
                let _ = app.update(Message::Remove("game".into()));
                let _ = app.update(Message::ConfirmRemove);
            }
            _ => {
                app.change = Some(RunningChange::Trash(crate::trash_page::Change::Prune {
                    ids: vec!["entry".into()],
                }))
            }
        }
        assert!(app.change.is_some());
        let generation = app.change_generation;
        let reason = app.change.as_ref().unwrap().reason();
        send(&mut app, Event::Fix(scope()));
        send(&mut app, Event::Fix(Scope::Global));
        let _ = app.update(Message::Remove("game".into()));
        let _ = app.update(Message::Trash(TrashEvent::Restore("entry".into())));
        let _ = app.update(Message::Trash(TrashEvent::Recover("entry".into())));
        let _ = app.update(Message::Trash(TrashEvent::Review(Selection::All)));
        assert_eq!(app.change_generation, generation);
        assert_eq!(app.change.as_ref().unwrap().reason(), reason);
        assert!(!app.trash_page.reviewing);
    }
}

#[test]
fn missing_scope_keys_are_safe_to_render() {
    let (_root, app) = fixture();
    assert!(app.checkups.is_empty());
    let _ = app.checkup_view(scope());
    let _ = app.checkup_view(Scope::Global);
    let _ = app.checkup_page();
}

#[test]
fn clean_page_uses_cli_environment_counts() {
    for (count, expected) in [
        (0, "No problems in 0 environments."),
        (1, "No problems in 1 environment."),
        (6, "No problems in 6 environments."),
    ] {
        let report = Report {
            environments: (0..count).map(|i| i.to_string()).collect(),
            ..Report::default()
        };
        assert_eq!(scope_summary(&report, &Scope::Global), expected);
        assert_eq!(scope_summary(&report, &scope()), "No problems found.");
    }
}

#[test]
fn page_check_failure_keeps_only_check_again_available() {
    let (_root, mut app) = fixture();
    page(&mut app);
    let generation = app.checkups[&Scope::Global].generation;
    send(
        &mut app,
        Event::Checked(Scope::Global, generation, Err("unavailable storage".into())),
    );
    assert_eq!(
        app.checkups[&Scope::Global].error.as_deref(),
        Some("unavailable storage")
    );
    assert!(app.checkups[&Scope::Global].report.is_none());
    assert_eq!(
        app.checkup_controls(&Scope::Global),
        [Scope::Global.id("check")]
    );
    let _ = app.checkup_page();
    send(&mut app, Event::Check(Scope::Global));
    accept(&mut app, Scope::Global, Report::default());
    assert!(app.checkups[&Scope::Global].error.is_none());
}
