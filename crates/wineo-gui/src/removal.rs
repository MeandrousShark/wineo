use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{Space, button, checkbox, column, container, mouse_area, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
use wineo_core::runtime::State;
use wineo_core::trash::{Entry, RemoveEvent};

use crate::{App, Message, remove_work, spinner};

pub(crate) struct Dialog {
    pub(crate) name: String,
    pub(crate) prefix: Option<PathBuf>,
    pub(crate) include_prefix: bool,
    pub(crate) size: Option<Result<u64, String>>,
    pub(crate) measure: Token,
    pub(crate) error: Option<String>,
}

pub(crate) enum Change {
    Remove {
        name: String,
        token: Token,
        phase: Option<RemoveEvent>,
    },
    Restore {
        id: String,
        name: String,
    },
}

impl Change {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Remove { name, .. } => format!("Busy: removing {name}"),
            Self::Restore { name, .. } => format!("Busy: restoring {name}"),
        }
    }
}

pub(crate) struct Toast {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) remaining: Duration,
    pub(crate) hovered: bool,
    pub(crate) focused: bool,
    last: Instant,
}

impl Toast {
    fn new(entry: Entry) -> Self {
        let mut text = format!("Removed {} to the trash.", entry.name);
        if entry.prefix_left.is_some() && entry.prefix_present == Some(true) {
            text.push_str(" Prefix kept.");
        }
        if entry.desktop_removed > 0 {
            text.push_str(&format!(
                " {} desktop entries removed.",
                entry.desktop_removed
            ));
        }
        Self {
            id: entry.id,
            name: entry.name,
            text,
            remaining: Duration::from_secs(10),
            hovered: false,
            focused: false,
            last: Instant::now(),
        }
    }

    pub(crate) fn advance(&mut self, now: Instant) {
        if !self.hovered && !self.focused {
            self.remaining = self
                .remaining
                .saturating_sub(now.saturating_duration_since(self.last));
        }
        self.last = now.max(self.last);
    }
}

impl App {
    pub(crate) fn remove_reason(&self, name: &str) -> Option<String> {
        if let Some(change) = &self.change {
            return Some(change.reason());
        }
        let card = self.cards.iter().find(|card| card.name == name)?;
        if card.lost || card.unresolved {
            Some("Stop it first. Use Clean up in the panel.".into())
        } else if card.state == State::Running
            || card.busy
            || self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.name == name)
        {
            Some("Stop it first".into())
        } else {
            None
        }
    }

    pub(crate) fn open_remove(&mut self, name: String) -> Task<Message> {
        if self.removal.is_some()
            || self.flow.is_some()
            || self.settings.is_some()
            || self.close_prompt
            || self.management_busy()
            || self.remove_reason(&name).is_some()
        {
            return Task::none();
        }
        let Some(card) = self.cards.iter().find(|card| card.name == name) else {
            return Task::none();
        };
        let prefix = card.prefix.clone();
        self.action_menu = None;
        self.remove_generation = self.remove_generation.wrapping_add(1);
        let measure = Token::new();
        let task = prefix.clone().map_or_else(Task::none, |path| {
            remove_work::measure(self.remove_generation, path, measure.clone())
        });
        self.removal = Some(Dialog {
            name,
            prefix,
            include_prefix: false,
            size: None,
            measure,
            error: None,
        });
        Task::batch([
            task,
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ])
    }

    pub(crate) fn confirm_remove(&mut self) -> Task<Message> {
        let Some(dialog) = &self.removal else {
            return Task::none();
        };
        if self.management_busy() || self.remove_reason(&dialog.name).is_some() {
            return Task::none();
        }
        let token = Token::new();
        let name = dialog.name.clone();
        let include_prefix = dialog.include_prefix && dialog.prefix.is_some();
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(Change::Remove {
            name: name.clone(),
            token: token.clone(),
            phase: None,
        });
        self.removal.as_mut().unwrap().error = None;
        remove_work::remove(
            self.change_generation,
            self.paths.clone(),
            name,
            include_prefix,
            token,
        )
    }

    pub(crate) fn cancel_management(&self) {
        if let Some(Change::Remove {
            token,
            phase: None | Some(RemoveEvent::Waiting),
            ..
        }) = &self.change
        {
            token.cancel();
        }
    }

    pub(crate) fn cancel_remove_dialog(&mut self) {
        if self.change.is_none()
            && let Some(dialog) = self.removal.take()
        {
            dialog.measure.cancel();
        }
    }

    pub(crate) fn cancel_remove(&mut self) {
        if self.change.is_some() {
            self.cancel_management();
        } else {
            self.cancel_remove_dialog();
        }
    }

    fn reload_after_change(&mut self) -> Task<Message> {
        self.refreshing = true;
        self.trash_refreshing = true;
        // Always queue fresh reads, even if a periodic read was already in flight.
        Task::batch([self.reload_library(), self.reload_badge()])
    }

    pub(crate) fn remove_finished(&mut self, result: Result<Entry, String>) -> Task<Message> {
        if !matches!(self.change, Some(Change::Remove { .. })) {
            return Task::none();
        }
        self.change = None;
        match result {
            Ok(entry) => {
                self.cancel_remove_dialog();
                self.close_panel();
                self.selected = None;
                self.toast = Some(Toast::new(entry));
            }
            Err(error) => {
                if let Some(dialog) = &mut self.removal {
                    dialog.error = Some(error);
                }
            }
        }
        self.reload_after_change()
    }

    pub(crate) fn undo_remove(&mut self) -> Task<Message> {
        if self.management_busy()
            || self.removal.is_some()
            || self.flow.is_some()
            || self.close_prompt
        {
            return Task::none();
        }
        let Some(toast) = &self.toast else {
            return Task::none();
        };
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(Change::Restore {
            id: toast.id.clone(),
            name: toast.name.clone(),
        });
        remove_work::restore(self.change_generation, self.paths.clone(), toast.id.clone())
    }

    pub(crate) fn undo_finished(&mut self, result: Result<(), String>) -> Task<Message> {
        if !matches!(self.change, Some(Change::Restore { .. })) {
            return Task::none();
        }
        let Some(Change::Restore { id, .. }) = self.change.take() else {
            return Task::none();
        };
        if let Some(toast) = &mut self.toast
            && toast.id == id
        {
            match result {
                Ok(()) => self.toast = None,
                Err(error) => {
                    toast.text = error;
                    toast.remaining = Duration::from_secs(10);
                    toast.last = Instant::now();
                }
            }
        }
        self.reload_after_change()
    }

    pub(crate) fn remove_view(&self) -> Element<'_, Message> {
        let dialog = self.removal.as_ref().unwrap();
        let palette = self.palette;
        let mut content = column![
            text(format!("Remove {}?", dialog.name)).size(22),
            text(
                "Its config, art, and playtime go to the trash.\nYou can restore them from Trash."
            )
        ]
        .spacing(16);
        if let Some(prefix) = &dialog.prefix {
            let size = match &dialog.size {
                None => "measuring...".into(),
                Some(Ok(bytes)) => size_label(*bytes),
                Some(Err(error)) => format!("Size unavailable: {error}"),
            };
            content = content
                .push(
                    checkbox(dialog.include_prefix)
                        .label("Also trash the prefix and snapshots")
                        .on_toggle_maybe(self.change.is_none().then_some(Message::RemovePrefix)),
                )
                .push(
                    text(format!(
                        "{} ({size})",
                        self.paths.shown(&prefix.to_string_lossy())
                    ))
                    .size(13)
                    .color(palette.muted),
                );
        }
        if let Some(error) = &dialog.error {
            content = content.push(text(error).color(palette.danger));
        }
        let moving = matches!(
            self.change,
            Some(Change::Remove {
                phase: Some(RemoveEvent::Moving),
                ..
            })
        );
        let running = self.change.is_some();
        if running {
            content = content.push(
                row![
                    spinner::spinner(palette, self.spinner_angle, 16.0),
                    text(if moving {
                        "Moving to the trash..."
                    } else {
                        if matches!(
                            self.change,
                            Some(Change::Remove {
                                phase: Some(RemoveEvent::Waiting),
                                ..
                            })
                        ) {
                            "Waiting for wine to exit"
                        } else {
                            "Removing..."
                        }
                    })
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        let reason = self.remove_reason(&dialog.name);
        if !running && let Some(reason) = &reason {
            content = content.push(text(reason.clone()).size(13).color(palette.muted));
        }
        let mut controls = row![
            Space::new().width(Fill),
            button("Cancel").on_press_maybe((!moving).then_some(Message::CancelRemove))
        ]
        .spacing(12);
        if !running {
            controls = controls.push(
                button("Remove").on_press_maybe(reason.is_none().then_some(Message::ConfirmRemove)),
            );
        }
        container(content.push(controls))
            .padding(24)
            .width((self.window_size.width - 40.0).clamp(280.0, 600.0))
            .style(move |_| {
                container::Style::default()
                    .background(palette.surface)
                    .border(border::rounded(8).width(1).color(palette.border))
            })
            .into()
    }

    pub(crate) fn toast_view(&self) -> Element<'_, Message> {
        let toast = self.toast.as_ref().unwrap();
        let palette = self.palette;
        let enabled = !self.management_busy() && self.flow.is_none() && self.removal.is_none();
        let undo = crate::focus_button::undo(
            button("Undo")
                .on_press_maybe(enabled.then_some(Message::Undo))
                .into(),
            enabled,
        );
        let mut content = row![
            text(&toast.text).wrapping(iced::widget::text::Wrapping::None),
            undo
        ]
        .spacing(12)
        .align_y(Center);
        if let Some(change) = &self.change {
            content = content.push(text(change.reason()).size(13));
        }
        mouse_area(container(content).padding(12).style(move |_| {
            container::Style::default()
                .background(palette.surface)
                .border(border::rounded(6).width(1).color(palette.border))
        }))
        .on_enter(Message::ToastHover(true))
        .on_exit(Message::ToastHover(false))
        .into()
    }
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operation, OperationKind, sidebar};
    use iced::event::Status;
    use iced::keyboard::{Key, Modifiers, key::Named};
    use wineo_core::paths::Paths;
    use wineo_core::trash;

    fn fixture(wine: bool) -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
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
}
