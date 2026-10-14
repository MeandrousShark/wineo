use super::*;
use crate::{Operation, OperationKind, sidebar};
use iced::event::Status;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::Paths;
use wineo_core::trash;

fn fixture(wine: bool) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    let config = paths.environment_config("game");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let content = if wine {
        let prefix = paths.default_prefix_root().join("game");
        std::fs::create_dir_all(&prefix).unwrap();
        format!(
            "prefix = '{}'\nrunner = 'missing-test-runner'\narch = 'win64'\n",
            prefix.display()
        )
    } else {
        "kind = 'native'\n".into()
    };
    std::fs::write(config, content).unwrap();
    let mut app = App::boot(paths).0;
    app.cards = crate::library::load_library(&app.paths).unwrap();
    app.refreshing = false;
    app.trash_refreshing = false;
    (root, app)
}

fn open(app: &mut App) {
    let _ = app.update(Message::Remove("game".into()));
    assert!(app.removal.is_some());
}

#[test]
fn canceled_remove_closes_quietly_but_real_errors_stay() {
    let (_root, mut app) = fixture(false);
    open(&mut app);
    let _ = app.update(Message::ConfirmRemove);
    let _ = app.update(Message::RemoveProgress(
        app.change_generation,
        RemoveEvent::Waiting,
    ));
    let _ = app.update(Message::CancelRemove);
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Err(remove_work::RemoveError::Canceled),
    ));
    assert!(app.removal.is_none() && app.change.is_none() && app.toast.is_none());
    open(&mut app);
    let _ = app.update(Message::ConfirmRemove);
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Err("real failure".into()),
    ));
    assert_eq!(
        app.removal.as_ref().unwrap().error.as_deref(),
        Some("real failure")
    );
}

#[test]
fn a_failed_remove_measures_the_prefix_again() {
    let (_root, mut app) = fixture(true);
    open(&mut app);
    let _ = app.update(Message::ConfirmRemove);
    assert!(app.removal.as_ref().unwrap().measure.is_canceled());
    let before = app.remove_generation;
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Err("real failure".into()),
    ));
    let dialog = app.removal.as_ref().unwrap();
    assert!(!dialog.measure.is_canceled());
    assert_ne!(app.remove_generation, before);
    let _ = app.update(Message::RemoveSize(app.remove_generation, Ok(42)));
    assert_eq!(app.removal.as_ref().unwrap().size, Some(Ok(42)));
}

#[test]
fn removing_another_card_keeps_selection_and_panel() {
    let (_root, mut app) = fixture(false);
    std::fs::write(app.paths.environment_config("other"), "kind = 'native'\n").unwrap();
    app.cards = crate::library::load_library(&app.paths).unwrap();
    app.selected = Some("other".into());
    app.panel_open = true;
    removed(&mut app);
    assert_eq!(app.selected.as_deref(), Some("other"));
    assert!(app.panel_open);
}

#[test]
fn confirming_cancels_measurement_and_ignores_its_result() {
    let (_root, mut app) = fixture(true);
    open(&mut app);
    let token = app.removal.as_ref().unwrap().measure.clone();
    let _ = app.update(Message::ConfirmRemove);
    assert!(token.is_canceled());
    let _ = app.update(Message::RemoveSize(
        app.remove_generation,
        Err("late measurement".into()),
    ));
    assert!(app.removal.as_ref().unwrap().size.is_none());
}

#[test]
fn settings_keep_control_z_and_block_undo() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let _ = app.update(Message::Section(crate::sidebar::Section::Settings));
    assert!(app.settings.is_some());
    let _ = app.update(Message::Key(
        Key::Character("z".into()),
        Modifiers::CTRL,
        Status::Ignored,
    ));
    assert!(app.change.is_none());
    let _ = app.update(Message::Undo);
    assert!(app.change.is_none());
    assert!(
        key(
            &mut app,
            &Key::Character("z".into()),
            Modifiers::CTRL,
            Status::Ignored
        )
        .is_none()
    );
}

#[test]
fn library_tab_is_unchanged_until_undo_is_available() {
    let (_root, mut app) = fixture(false);
    for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
        assert!(
            key(
                &mut app,
                &Key::Named(Named::Tab),
                modifiers,
                Status::Ignored
            )
            .is_none()
        );
        assert_eq!(
            crate::keys::action(
                &Key::Named(Named::Tab),
                modifiers,
                false,
                false,
                false,
                true
            ),
            crate::keys::Action::None
        );
    }
    removed(&mut app);
    assert!(
        key(
            &mut app,
            &Key::Named(Named::Tab),
            Modifiers::empty(),
            Status::Ignored
        )
        .is_some()
    );
}

#[test]
fn dialog_focus_controls_match_each_phase() {
    let (_root, mut app) = fixture(true);
    open(&mut app);
    assert_eq!(app.remove_controls(), [PREFIX_ID, CANCEL_ID, REMOVE_ID]);
    let _ = app.update(Message::ConfirmRemove);
    assert_eq!(app.remove_controls(), [CANCEL_ID]);
    let _ = app.update(Message::RemoveProgress(
        app.change_generation,
        RemoveEvent::Moving,
    ));
    assert!(app.remove_controls().is_empty());
    let (_root, mut app) = fixture(false);
    open(&mut app);
    assert_eq!(app.remove_controls(), [CANCEL_ID, REMOVE_ID]);
}

#[test]
fn delete_focus_distinguishes_search_from_undo() {
    use iced::advanced::widget::{Operation, operation::Focusable};
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
    for search in [false, true] {
        let mut query = RemoveFocus::default();
        query.focusable(
            Some(&iced::widget::Id::new(crate::SEARCH_ID)),
            iced::Rectangle::default(),
            &mut Focus(search),
        );
        query.focusable(
            Some(&iced::widget::Id::new(focus_button::UNDO_ID)),
            iced::Rectangle::default(),
            &mut Focus(!search),
        );
        let (_root, mut app) = fixture(false);
        app.selected = Some("game".into());
        app.search_focused = !search;
        let iced::advanced::widget::operation::Outcome::Some((actual_search, any)) = query.finish()
        else {
            panic!("missing focus result")
        };
        let _ = app.update(Message::RemoveFocus(Status::Ignored, actual_search, any));
        assert_eq!(app.search_focused, search);
        assert!(app.removal.is_none());
    }
}

#[test]
fn toast_pluralizes_desktop_entries_and_mentions_retained_snapshots() {
    let (_root, mut app) = fixture(true);
    let mut entry = removed(&mut app);
    let snapshots = app.paths.default_prefix_root().join("kept-snapshots");
    std::fs::create_dir_all(&snapshots).unwrap();
    entry.snapshots_left = Some(snapshots.clone());
    assert!(!Toast::new(entry.clone()).text.contains("Snapshots kept."));
    std::fs::write(snapshots.join("snapshot"), "snapshot").unwrap();
    for count in [1, 2] {
        entry.desktop_removed = count;
        let toast = Toast::new(entry.clone());
        assert!(toast.text.contains(if count == 1 {
            "1 desktop entry removed."
        } else {
            "2 desktop entries removed."
        }));
        assert!(toast.text.contains("Prefix kept.") && toast.text.contains("Snapshots kept."));
        assert!(!toast.text.contains('\n'));
    }
}

#[test]
fn long_undo_errors_wrap_even_with_an_unbroken_path() {
    use iced::advanced::text::{self as advanced_text, Paragraph as _};
    type Paragraph = <iced::Renderer as advanced_text::Renderer>::Paragraph;
    let error = format!("Restore failed at /{}", "long-directory-name".repeat(32));
    let paragraph = Paragraph::with_text(advanced_text::Text {
        content: error.as_str(),
        bounds: iced::Size::new(240.0, f32::INFINITY),
        size: 16.0.into(),
        line_height: advanced_text::LineHeight::default(),
        font: iced::Font::DEFAULT,
        align_x: advanced_text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: advanced_text::Shaping::default(),
        wrapping: TOAST_WRAPPING,
    });
    assert!(paragraph.min_width() <= 240.0);
    assert!(paragraph.min_height() > 32.0);
}

#[test]
fn expiry_during_restore_keeps_toast_and_double_undo_is_blocked() {
    let (_root, mut app) = fixture(false);
    let entry = removed(&mut app);
    let start = app.toast.as_ref().unwrap().last;
    let _ = app.update(Message::Undo);
    let generation = app.change_generation;
    let _ = app.update(Message::Undo);
    assert_eq!(app.change_generation, generation);
    assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(11)));
    assert!(app.toast.as_ref().unwrap().remaining.is_zero());
    let _ = app.update(Message::UndoFinished(
        generation,
        Err("restore failed".into()),
    ));
    assert_eq!(
        app.toast.as_ref().unwrap().remaining,
        Duration::from_secs(10)
    );
}

#[test]
fn prefix_defaults_off_and_native_has_no_checkbox() {
    for wine in [false, true] {
        let (_root, mut app) = fixture(wine);
        open(&mut app);
        assert!(!app.removal.as_ref().unwrap().include_prefix);
        assert_eq!(app.removal.as_ref().unwrap().prefix.is_some(), wine);
        let _ = app.update(Message::RemovePrefix(true));
        assert_eq!(app.removal.as_ref().unwrap().include_prefix, wine);
        let _ = app.update(Message::ConfirmRemove);
        assert!(matches!(app.change, Some(Change::Remove { .. })));
    }
}

#[test]
fn remove_refuses_running_busy_lost_unresolved_and_in_flight() {
    for state in 0..5 {
        let (_root, mut app) = fixture(false);
        match state {
            0 => app.cards[0].state = State::Running,
            1 => app.cards[0].busy = true,
            2 => app.cards[0].lost = true,
            3 => app.cards[0].unresolved = true,
            _ => {
                app.operation = Some(Operation {
                    name: "game".into(),
                    kind: OperationKind::Stop,
                    token: None,
                    cancelled: false,
                    status: String::new(),
                    install_done: None,
                    installing: false,
                    after_hook: false,
                })
            }
        }
        assert!(
            app.remove_reason("game")
                .unwrap()
                .starts_with("Stop it first")
        );
        let _ = app.update(Message::Remove("game".into()));
        assert!(app.removal.is_none());
    }
}

#[test]
fn cancel_wait_and_moving_and_one_change_at_a_time() {
    for moving in [false, true] {
        let (_root, mut app) = fixture(false);
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        let token = match &app.change {
            Some(Change::Remove { token, .. }) => token.clone(),
            _ => unreachable!(),
        };
        let _ = app.update(Message::RemoveProgress(
            app.change_generation,
            if moving {
                RemoveEvent::Moving
            } else {
                RemoveEvent::Waiting
            },
        ));
        let _ = app.update(Message::ConfirmRemove);
        let _ = app.update(Message::Undo);
        let _ = app.update(Message::Section(sidebar::Section::Trash));
        assert_eq!(app.section, sidebar::Section::Library);
        let _ = app.update(Message::CancelRemove);
        assert_eq!(token.is_canceled(), !moving);
        assert!(app.removal.is_some());
        assert_eq!(app.change.as_ref().unwrap().reason(), "Busy: removing game");
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Err("failed".into()),
        ));
        assert!(app.change.is_none());
        assert_eq!(
            app.removal.as_ref().unwrap().error.as_deref(),
            Some("failed")
        );
        assert!(app.refreshing && app.trash_refreshing);
    }
}

fn removed(app: &mut App) -> Entry {
    open(app);
    let _ = app.update(Message::ConfirmRemove);
    let entry = trash::remove(&app.paths, "game", false).unwrap();
    let _ = app.update(Message::RemoveFinished(
        app.change_generation,
        Ok(entry.clone()),
    ));
    entry
}

#[test]
fn success_closes_panel_deselects_and_reloads_with_undo_id() {
    let (_root, mut app) = fixture(true);
    app.selected = Some("game".into());
    app.panel_open = true;
    let entry = removed(&mut app);
    assert!(app.removal.is_none() && !app.panel_open && app.selected.is_none());
    assert!(app.refreshing && app.trash_refreshing);
    let toast = app.toast.as_ref().unwrap();
    assert_eq!(toast.id, entry.id);
    assert!(toast.text.starts_with("Removed game to the trash."));
    assert!(toast.text.contains("Prefix kept."));
}

#[test]
fn successful_remove_clears_only_that_environments_checkup() {
    let (_root, mut app) = fixture(false);
    let scope = crate::checkup::Scope::Environment("game".into());
    let other = crate::checkup::Scope::Environment("other".into());
    let _ = app.update(Message::Checkup(crate::checkup::Event::Open(
        crate::checkup::Scope::Environment("game".into()),
    )));
    app.checkups.insert(other.clone(), Default::default());
    assert!(app.checkups.contains_key(&scope));
    let entry = removed(&mut app);
    assert!(!app.checkups.contains_key(&scope));
    assert!(app.checkups.contains_key(&other));
    let _ = app.update(Message::Undo);
    trash::restore(&app.paths, &entry.id).unwrap();
    let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
    let _ = app.update(Message::Loaded(Ok(crate::library::load_library(
        &app.paths,
    )
    .unwrap())));
    let _ = app.update(Message::Select("game".into()));
    assert!(app.panel_open && !app.checkup_visible());
    assert!(!app.checkups.contains_key(&scope));
}

#[test]
fn undo_uses_entry_id_refusal_stays_and_success_dismisses() {
    let (_root, mut app) = fixture(false);
    let entry = removed(&mut app);
    let _ = app.update(Message::Undo);
    assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
    let _ = app.update(Message::Remove("game".into()));
    assert!(app.removal.is_none());
    let config = app.paths.environment_config("game");
    std::fs::write(&config, "kind = 'native'\n").unwrap();
    let error = trash::restore(&app.paths, &entry.id)
        .unwrap_err()
        .to_string();
    app.refreshing = false;
    app.trash_refreshing = false;
    let _ = app.update(Message::UndoFinished(
        app.change_generation,
        Err(error.clone()),
    ));
    assert_eq!(app.toast.as_ref().unwrap().text, error);
    assert!(app.change.is_none() && app.refreshing && app.trash_refreshing);
    std::fs::remove_file(config).unwrap();
    let _ = app.update(Message::Key(
        Key::Character("z".into()),
        Modifiers::CTRL,
        Status::Ignored,
    ));
    assert!(matches!(app.change, Some(Change::Restore { .. })));
    trash::restore(&app.paths, &entry.id).unwrap();
    let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
    assert!(app.toast.is_none());
}

#[test]
fn toast_expiry_pauses_on_hover_and_focus() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let start = app.toast.as_ref().unwrap().last;
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(4)));
    assert_eq!(
        app.toast.as_ref().unwrap().remaining,
        Duration::from_secs(6)
    );
    let _ = app.update(Message::ToastHover(true));
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(24)));
    assert_eq!(
        app.toast.as_ref().unwrap().remaining,
        Duration::from_secs(6)
    );
    let _ = app.update(Message::ToastHover(false));
    let _ = app.update(Message::ToastFocus(true, start + Duration::from_secs(44)));
    assert_eq!(
        app.toast.as_ref().unwrap().remaining,
        Duration::from_secs(6)
    );
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(64)));
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(70)));
    assert!(app.toast.is_none());
}

#[test]
fn toast_has_one_wakeup_at_expiry_and_none_while_paused() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let toast = app.toast.as_ref().unwrap();
    let expiry = toast.last + toast.remaining;
    assert_eq!(app.toast_wake.due(), Some(expiry));

    let _ = app.update(Message::ToastHover(true));
    assert!(app.toast_wake.due().is_none(), "hover leaves no timer");
    let paused = app.toast.as_ref().unwrap().remaining;

    let _ = app.update(Message::ToastHover(false));
    let toast = app.toast.as_ref().unwrap();
    assert_eq!(
        toast.remaining, paused,
        "unpausing keeps the remaining time"
    );
    assert_eq!(app.toast_wake.due(), Some(toast.last + toast.remaining));

    let start = toast.last;
    let _ = app.update(Message::ToastFocus(true, start + Duration::from_secs(1)));
    assert!(app.toast_wake.due().is_none(), "focus leaves no timer");
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(2)));
    let toast = app.toast.as_ref().unwrap();
    assert_eq!(app.toast_wake.due(), Some(toast.last + toast.remaining));
}

#[test]
fn a_stale_toast_wakeup_changes_nothing_and_the_live_one_checks_focus() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let live = app.toast_wake.generation();
    let remaining = app.toast.as_ref().unwrap().remaining;

    let _ = app.update(Message::ToastTick(live.wrapping_sub(1)));
    assert!(app.toast_wake.due().is_some());
    assert_eq!(app.toast.as_ref().unwrap().remaining, remaining);

    let _ = app.update(Message::ToastTick(live));
    // The wakeup was consumed and nothing re-arms until the focus answer
    // comes back.
    assert!(app.toast_wake.due().is_none());
    let _ = app.update(Message::ToastFocus(false, Instant::now()));
    assert!(app.toast_wake.due().is_some());
}

#[test]
fn an_expired_toast_waits_for_the_change_and_then_goes() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let _ = app.update(Message::Undo);
    let start = app.toast.as_ref().unwrap().last;
    let _ = app.update(Message::ToastFocus(false, start + Duration::from_secs(11)));
    assert!(app.toast.as_ref().unwrap().remaining.is_zero());
    assert!(app.toast_wake.due().is_none(), "nothing left to wait for");

    app.change = None;
    let _ = app.update(Message::ToastHover(false));
    assert!(app.toast.is_none());
}

#[test]
fn the_feature_notice_gets_its_own_wakeup() {
    let (_root, mut app) = fixture(false);
    let until = Instant::now() + Duration::from_secs(8);
    app.feature_notice = Some(("Done".into(), until));
    let _ = app.update(Message::ToastHover(false));
    assert_eq!(app.toast_wake.due(), Some(until));

    removed(&mut app);
    let toast = app.toast.as_ref().unwrap();
    let expiry = toast.last + toast.remaining;
    assert_eq!(app.toast_wake.due(), Some(expiry.min(until)));

    let live = app.toast_wake.generation();
    app.feature_notice = Some(("Done".into(), Instant::now() - Duration::from_millis(1)));
    let _ = app.update(Message::ToastTick(live));
    assert!(app.feature_notice.is_none());
}

#[test]
fn the_expiry_wakeup_asks_about_focus_before_it_expires_the_toast() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    // Undo gained focus (a held Tab) after the last check, just before
    // the deadline passed.
    let toast = app.toast.as_mut().unwrap();
    toast.last = Instant::now() - Duration::from_secs(11);
    let live = app.toast_wake.generation();

    let _ = app.update(Message::ToastTick(live));
    assert!(app.toast.is_some(), "expired before the answer arrived");
    assert!(!app.toast.as_ref().unwrap().remaining.is_zero());
    assert!(app.toast_wake.due().is_none(), "no timer while it waits");

    let _ = app.update(Message::ToastFocus(true, Instant::now()));
    let toast = app.toast.as_ref().unwrap();
    assert!(toast.focused && !toast.remaining.is_zero());
    assert!(app.toast_wake.due().is_none());

    // Focus left again: the first answer ends the pause, the next one
    // finds the clock run out.
    let later = Instant::now() + Duration::from_secs(11);
    let _ = app.update(Message::ToastFocus(false, Instant::now()));
    let _ = app.update(Message::ToastFocus(false, later));
    assert!(app.toast.is_none());
}

#[test]
fn the_expiry_wakeup_expires_an_unfocused_toast_on_the_answer() {
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    app.toast.as_mut().unwrap().last = Instant::now() - Duration::from_secs(11);
    let live = app.toast_wake.generation();
    let _ = app.update(Message::ToastTick(live));
    let _ = app.update(Message::ToastFocus(false, Instant::now()));
    assert!(app.toast.is_none());
}

#[test]
fn a_focus_answer_for_a_replaced_toast_is_ignored() {
    let (_root, mut app) = fixture(false);
    let entry = removed(&mut app);
    let old = app.toast.as_ref().unwrap().serial;
    app.toast = Some(Toast::new(entry));
    let toast = app.toast.as_ref().unwrap();
    assert_ne!(toast.serial, old);
    let (remaining, last) = (toast.remaining, toast.last);

    let _ = app.update(Message::ToastAnswer(
        old,
        true,
        last + Duration::from_secs(30),
    ));
    let toast = app.toast.as_ref().unwrap();
    assert!(!toast.focused && toast.remaining == remaining && toast.last == last);

    let live = toast.serial;
    let _ = app.update(Message::ToastAnswer(live, true, last));
    assert!(app.toast.as_ref().unwrap().focused);
}

#[test]
fn focus_is_rechecked_after_a_click_a_key_release_or_any_change_while_focused() {
    let (_root, mut app) = fixture(false);
    assert!(!app.toast_focus_may_have_changed(&Message::Pressed));
    removed(&mut app);
    assert!(app.toast_focus_may_have_changed(&Message::Pressed));
    assert!(app.toast_focus_may_have_changed(&Message::FocusRecheck));
    assert!(!app.toast_focus_may_have_changed(&Message::Rebuild));
    app.toast.as_mut().unwrap().focused = true;
    assert!(app.toast_focus_may_have_changed(&Message::Rebuild));
    assert!(!app.toast_focus_may_have_changed(&Message::ToastFocus(true, Instant::now())));
    assert!(!app.toast_focus_may_have_changed(&Message::ScrollIdle(0)));
}

#[test]
fn delete_and_escape_respect_focus_section_modal_and_running_state() {
    for blocked in 0..6 {
        let (_root, mut app) = fixture(false);
        app.selected = Some("game".into());
        if blocked == 1 {
            app.section = sidebar::Section::Trash;
        }
        if blocked == 2 {
            app.log_view = true;
        }
        if blocked == 3 {
            app.close_prompt = true;
        }
        let _ = app.update(Message::KeyWithFocus(
            Key::Named(Named::Delete),
            Modifiers::empty(),
            if blocked == 4 {
                Status::Captured
            } else {
                Status::Ignored
            },
            blocked == 5,
        ));
        assert_eq!(app.removal.is_some(), blocked == 0);
        if blocked == 0 {
            let measure = app.removal.as_ref().unwrap().measure.clone();
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                Status::Ignored,
            ));
            assert!(app.removal.is_none() && measure.is_canceled());
            open(&mut app);
            let _ = app.update(Message::ConfirmRemove);
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                Status::Ignored,
            ));
            assert!(app.removal.is_some());
        }
    }
}

#[test]
fn mouse_back_dismisses_the_remove_dialog_like_escape() {
    let (_root, mut app) = fixture(false);
    app.selected = Some("game".into());
    open(&mut app);
    let measure = app.removal.as_ref().unwrap().measure.clone();
    crate::tests::press_back(&mut app, Status::Captured);
    assert!(app.removal.is_none() && measure.is_canceled());
}

#[test]
fn closing_prompts_cancels_wait_and_waits_for_moving_or_restore() {
    for moving in [false, true] {
        let (_root, mut app) = fixture(false);
        open(&mut app);
        let _ = app.update(Message::ConfirmRemove);
        if moving {
            let _ = app.update(Message::RemoveProgress(
                app.change_generation,
                RemoveEvent::Moving,
            ));
        }
        let token = match &app.change {
            Some(Change::Remove { token, .. }) => token.clone(),
            _ => unreachable!(),
        };
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt && !app.closing);
        let _ = app.update(Message::KeepWorking);
        assert!(!app.close_prompt);
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy());
        assert_eq!(token.is_canceled(), !moving);
        let _ = app.update(Message::RemoveFinished(
            app.change_generation,
            Err("failed".into()),
        ));
        assert!(!app.foreground_busy());
    }
    let (_root, mut app) = fixture(false);
    removed(&mut app);
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    let _ = app.update(Message::CancelAndClose);
    assert!(app.closing && app.foreground_busy());
    let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
    assert!(!app.foreground_busy());
}

#[test]
fn stale_reads_and_progress_cannot_replace_a_new_change() {
    let (_root, mut app) = fixture(false);
    let old_library = app.library_generation;
    let old_badge = app.trash_generation;
    let stale_cards = app.cards.clone();
    let entry = removed(&mut app);
    let old_change = app.change_generation;
    let _ = app.update(Message::LibraryReloaded(
        app.library_generation,
        Ok(Vec::new()),
    ));
    let fresh_badge = sidebar::TrashBadge {
        count: Some(1),
        warning: false,
    };
    let _ = app.update(Message::TrashReloaded(
        app.trash_generation,
        fresh_badge.clone(),
    ));
    let _ = app.update(Message::LibraryReloaded(old_library, Ok(stale_cards)));
    let _ = app.update(Message::TrashReloaded(
        old_badge,
        sidebar::TrashBadge::default(),
    ));
    assert!(app.cards.is_empty());
    assert_eq!(app.trash_badge, fresh_badge);
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::RemoveProgress(old_change, RemoveEvent::Moving));
    let _ = app.update(Message::RemoveFinished(old_change, Err("late".into())));
    assert!(matches!(&app.change, Some(Change::Restore { id, .. }) if id == &entry.id));
    trash::restore(&app.paths, &entry.id).unwrap();
    let _ = app.update(Message::UndoFinished(app.change_generation, Ok(())));
    app.cards = crate::library::load_library(&app.paths).unwrap();
    open(&mut app);
    let _ = app.update(Message::ConfirmRemove);
    let _ = app.update(Message::RemoveProgress(old_change, RemoveEvent::Moving));
    assert!(matches!(
        app.change,
        Some(Change::Remove { phase: None, .. })
    ));
    let _ = app.update(Message::UndoFinished(old_change + 1, Ok(())));
    assert!(matches!(app.change, Some(Change::Remove { .. })));
}

#[test]
fn another_games_launch_does_not_block_removing_an_idle_card() {
    let (_root, mut app) = fixture(false);
    app.operation = Some(Operation {
        name: "other".into(),
        kind: OperationKind::Start,
        token: Some(Token::new()),
        cancelled: false,
        status: String::new(),
        install_done: None,
        installing: false,
        after_hook: false,
    });
    assert!(app.remove_reason("game").is_none());
    open(&mut app);
    let _ = app.update(Message::ConfirmRemove);
    assert!(matches!(app.change, Some(Change::Remove { .. })));
}

#[test]
fn closing_a_dialog_cancels_measurement_and_ignores_old_size() {
    let (_root, mut app) = fixture(true);
    open(&mut app);
    let old = app.remove_generation;
    let token = app.removal.as_ref().unwrap().measure.clone();
    let _ = app.update(Message::CancelRemove);
    assert!(token.is_canceled());
    open(&mut app);
    let _ = app.update(Message::RemoveSize(old, Ok(42)));
    assert!(app.removal.as_ref().unwrap().size.is_none());
    let _ = app.update(Message::RemoveSize(app.remove_generation, Ok(24)));
    assert_eq!(app.removal.as_ref().unwrap().size, Some(Ok(24)));
    let token = app.removal.as_ref().unwrap().measure.clone();
    let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
    assert!(app.removal.is_none() && token.is_canceled());
}
