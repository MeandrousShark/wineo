use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{Space, checkbox, column, container, mouse_area, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::cancel::Token;
#[cfg(test)]
use wineo_core::runtime::State;
use wineo_core::trash::{Entry, RemoveEvent};

use crate::prefix_facts::{Facts, Live};
use crate::ui::{accent_base, secondary_button};
use crate::{App, Message, focus_button, remove_work, spinner};

const PREFIX_ID: &str = "remove-prefix";
const CANCEL_ID: &str = "remove-cancel";
const REMOVE_ID: &str = "remove-confirm";
const TOAST_WRAPPING: iced::widget::text::Wrapping = iced::widget::text::Wrapping::WordOrGlyph;

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
    Trash(crate::trash_page::Change),
    Downloads(crate::downloads_page::Change),
    Checkup(crate::checkup::Change),
}

impl Change {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Remove { name, .. } => format!("Busy: removing {name}"),
            Self::Restore { name, .. } => format!("Busy: restoring {name}"),
            Self::Trash(change) => change.reason(),
            Self::Downloads(change) => change.reason(),
            Self::Checkup(change) => format!("Busy: fixing {}", change.scope.label()),
        }
    }
}

pub(crate) struct Toast {
    pub(crate) id: String,
    /// Tells this toast from a later one, for answers still in flight.
    serial: u64,
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) remaining: Duration,
    pub(crate) hovered: bool,
    pub(crate) focused: bool,
    /// The expiry wakeup fired and the focus answer that decides it is still
    /// on its way, so there is nothing to arm a timer for.
    checking: bool,
    last: Instant,
    /// The entry is back and the toast is only telling what the restore had
    /// to say, so there is nothing left to undo.
    restored: bool,
}

impl Toast {
    pub(crate) fn new(entry: Entry) -> Self {
        let mut text = format!("Removed {} to the trash.", entry.name);
        if entry.prefix_left.is_some() && entry.prefix_present == Some(true) {
            text.push_str(" Prefix kept.");
        }
        if entry.desktop_removed > 0 {
            text.push_str(&format!(
                " {} desktop entr{} removed.",
                entry.desktop_removed,
                if entry.desktop_removed == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }
        for warning in &entry.warnings {
            text.push_str(&format!(
                " Warning: {}",
                wineo_core::error::one_line(warning)
            ));
        }
        if let Some(snapshots) = &entry.snapshots_left
            && snapshots.is_dir()
            && std::fs::read_dir(snapshots).is_ok_and(|mut entries| entries.next().is_some())
        {
            text.push_str(" Snapshots kept.");
        }
        static NEXT_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            id: entry.id,
            serial: NEXT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name: entry.name,
            text,
            remaining: Duration::from_secs(10),
            hovered: false,
            focused: false,
            checking: false,
            last: Instant::now(),
            restored: false,
        }
    }

    /// When the toast expires, if its clock is running. Hover and focus
    /// pause it, and an expired one has nothing left to wait for.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        (!self.hovered && !self.focused && !self.checking && !self.remaining.is_zero())
            .then(|| self.last + self.remaining)
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

pub(crate) fn key(
    app: &mut App,
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    status: iced::event::Status,
) -> Option<Task<Message>> {
    use iced::keyboard::{Key, Modifiers, key::Named};
    if app.settings.is_some() || app.art_page.is_some() {
        return None;
    }
    if status != iced::event::Status::Captured
        && app.toast.is_some()
        && modifiers == Modifiers::CTRL
        && matches!(key.as_ref(), Key::Character("z" | "Z"))
    {
        return Some(app.undo_remove());
    }
    if *key == Key::Named(Named::Tab) && app.toast.is_some() {
        return Some(
            iced::advanced::widget::operate(focus_button::cycle(
                &[focus_button::UNDO_ID],
                modifiers.shift(),
            ))
            .discard(),
        );
    }
    if *key == Key::Named(Named::Delete) && modifiers.is_empty() {
        return Some(
            iced::advanced::widget::operate(RemoveFocus::default())
                .map(move |(search, any)| Message::RemoveFocus(status, search, any)),
        );
    }
    None
}

#[derive(Default)]
struct RemoveFocus {
    search: bool,
    any: bool,
}

impl iced::advanced::widget::Operation<(bool, bool)> for RemoveFocus {
    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<(bool, bool)>),
    ) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: iced::Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if state.is_focused() {
            self.any = true;
            self.search |= id == Some(&iced::widget::Id::new(crate::SEARCH_ID));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<(bool, bool)> {
        iced::advanced::widget::operation::Outcome::Some((self.search, self.any))
    }
}

pub(crate) fn delete_key(app: &mut App, search: bool, captured: bool) -> Task<Message> {
    if !search
        && !captured
        && !app.panel_log_visible()
        && !app.version_visible()
        && !app.checkup_visible()
        && app.action_menu.is_none()
        && !app.sort_menu
    {
        app.selected
            .clone()
            .map_or_else(Task::none, |name| app.open_remove(name))
    } else {
        Task::none()
    }
}

pub(crate) fn dialog_key(
    app: &mut App,
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Task<Message> {
    use iced::keyboard::{Key, key::Named};
    match key {
        Key::Named(Named::Escape) => {
            app.cancel_remove_dialog();
            Task::none()
        }
        Key::Named(Named::Tab) => {
            let ids = app.remove_controls();
            iced::advanced::widget::operate(focus_button::cycle(&ids, modifiers.shift())).discard()
        }
        _ => Task::none(),
    }
}

pub(crate) fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::RemoveFocus(status, search, any) => {
            app.search_focused = search;
            if app.section == crate::sidebar::Section::Library
                && app.settings.is_none()
                && app.art_page.is_none()
                && app.flow.is_none()
                && app.removal.is_none()
                && app.hook_stop_confirm.is_none()
                && !app.close_prompt
            {
                delete_key(app, search, any || status == iced::event::Status::Captured)
            } else {
                Task::none()
            }
        }
        Message::Remove(name) => app.open_remove(name),
        Message::RemovePrefix(include) => {
            if app.change.is_none()
                && let Some(dialog) = &mut app.removal
            {
                dialog.include_prefix = include && dialog.prefix.is_some();
            }
            Task::none()
        }
        Message::RemoveSize(generation, size) => {
            if generation == app.remove_generation
                && app.change.is_none()
                && let Some(dialog) = &mut app.removal
            {
                dialog.size = Some(size);
            }
            Task::none()
        }
        Message::ConfirmRemove => app.confirm_remove(),
        Message::CancelRemove => {
            app.cancel_remove();
            Task::none()
        }
        Message::RemoveProgress(generation, event) => {
            if generation != app.change_generation {
                return Task::none();
            }
            if let Some(Change::Remove { phase, .. }) = &mut app.change {
                *phase = Some(event);
            }
            Task::none()
        }
        Message::RemoveFinished(generation, result) if generation == app.change_generation => {
            app.remove_finished(result)
        }
        Message::RemoveFinished(..) => Task::none(),
        Message::Undo => app.undo_remove(),
        Message::UndoFinished(generation, result) if generation == app.change_generation => {
            app.undo_finished(result)
        }
        Message::UndoFinished(..) => Task::none(),
        Message::ToastHover(hovered) => {
            if let Some(toast) = &mut app.toast {
                toast.advance(Instant::now());
                toast.hovered = hovered;
            }
            Task::none()
        }
        Message::ToastTick(generation) => {
            if app.toast_wake.fired(generation) {
                // Expiry is decided by the answer, not by the clock: Undo may
                // have gained focus since the last check, and that pauses it.
                if let Some(toast) = &mut app.toast {
                    toast.checking = true;
                }
                app.query_toast_focus(Instant::now())
            } else {
                Task::none()
            }
        }
        // An answer about a toast that has since been replaced is dropped.
        Message::ToastAnswer(serial, focused, now) => {
            if app
                .toast
                .as_ref()
                .is_some_and(|toast| toast.serial == serial)
            {
                update(app, Message::ToastFocus(focused, now))
            } else {
                Task::none()
            }
        }
        Message::ToastFocus(focused, now) => {
            if let Some(toast) = &mut app.toast {
                // Focus operations run before this message, so preserve the interval that gained focus.
                if focused {
                    toast.focused = true;
                }
                toast.advance(now);
                toast.focused = focused;
                toast.checking = false;
                if toast.remaining.is_zero() && app.change.is_none() {
                    app.toast = None;
                }
            }
            Task::none()
        }
        _ => Task::none(),
    }
}

impl App {
    /// Focus is only known by asking the widget tree, and nothing tells the
    /// toast when Undo gains or loses it, so it asks after the events that
    /// can move it: a click, a key coming up, and any change to the screen
    /// while Undo holds focus (it may have been disabled).
    pub(crate) fn toast_focus_may_have_changed(&self, message: &Message) -> bool {
        let Some(toast) = &self.toast else {
            return false;
        };
        match message {
            Message::Pressed | Message::FocusRecheck => true,
            Message::ToastTick(_)
            | Message::ToastHover(_)
            | Message::ToastFocus(..)
            | Message::ToastAnswer(..) => false,
            // Frame-rate and timer messages never move focus.
            Message::PanelTick(..)
            | Message::GridTick(..)
            | Message::CollapseTick(..)
            | Message::ScrollIdle(_) => false,
            _ => toast.focused,
        }
    }

    pub(crate) fn poll_toast_focus(&mut self) -> Task<Message> {
        let Some(toast) = &mut self.toast else {
            return Task::none();
        };
        let now = Instant::now();
        // Charge the time so far to the state the toast was last known to be
        // in, so a focus change found now only pauses from here on.
        if !toast.checking {
            toast.advance(now);
        }
        self.query_toast_focus(now)
    }

    fn query_toast_focus(&self, now: Instant) -> Task<Message> {
        let Some(toast) = &self.toast else {
            return Task::none();
        };
        let serial = toast.serial;
        iced::advanced::widget::operate(crate::focus_button::undo_focused())
            .map(move |focused| Message::ToastAnswer(serial, focused, now))
    }

    /// Drops a toast that ran out while a change blocked its removal, once
    /// the change is over.
    pub(crate) fn settle_toast(&mut self) {
        if self.change.is_none()
            && self
                .toast
                .as_ref()
                .is_some_and(|toast| toast.remaining.is_zero())
        {
            self.toast = None;
        }
    }

    /// The next time the toast or the feature notice needs the app to wake.
    pub(crate) fn next_toast_deadline(&self) -> Option<Instant> {
        let toast = self.toast.as_ref().and_then(Toast::deadline);
        let notice = self.feature_notice.as_ref().map(|(_, until)| *until);
        [toast, notice].into_iter().flatten().min()
    }

    pub(crate) fn remove_reason(&self, name: &str) -> Option<String> {
        self.remove_reason_with(&Live(&self.paths), name)
    }

    /// `view` passes the stored facts so a redraw never touches the disk.
    pub(crate) fn remove_reason_with(&self, facts: &dyn Facts, name: &str) -> Option<String> {
        if facts.restore_pending(name) {
            return Some("Roll back restore first.".into());
        }
        self.prefix_change_reason_with(facts, name)
    }

    pub(crate) fn prefix_change_reason(&self, name: &str) -> Option<String> {
        self.prefix_change_reason_with(&Live(&self.paths), name)
    }

    pub(crate) fn prefix_change_reason_with(
        &self,
        facts: &dyn Facts,
        name: &str,
    ) -> Option<String> {
        if self.filesystem_busy() {
            return Some("Busy: changing files.".into());
        }
        if let Some(reason) = self.tool_reason_with(facts, name) {
            return Some(reason);
        }
        if let Some(change) = &self.change {
            return Some(change.reason());
        }
        if let Some(other) = self.cards.iter().find(|card| {
            card.name != name
                && self.same_prefix_with(facts, name, &card.name)
                && (card.lost || card.unresolved || card.active() || card.busy)
        }) {
            return Some(if other.lost || other.unresolved {
                "Stop it first. Use Clean up in the panel.".into()
            } else {
                "Stop it first".into()
            });
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| self.same_prefix_with(facts, name, &operation.name))
        {
            return Some("Stop it first".into());
        }
        let card = self.cards.iter().find(|card| card.name == name)?;
        if card.lost || card.unresolved {
            Some("Stop it first. Use Clean up in the panel.".into())
        } else if card.active()
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
            || self.art_page.is_some()
            || self.close_prompt
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
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
        dialog.measure.cancel();
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
        self.cancel_checkup(false);
        if let Some(Change::Downloads(crate::downloads_page::Change::Download { token, .. })) =
            &self.change
        {
            token.cancel();
        }
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

    pub(crate) fn reload_after_change(&mut self) -> Task<Message> {
        self.refreshing = true;
        self.trash_refreshing = true;
        // Always queue fresh reads, even if a periodic read was already in flight.
        Task::batch([self.reload_library(), self.reload_trash_page()])
    }

    pub(crate) fn reload_after_uses_change(&mut self) -> Task<Message> {
        Task::batch([self.reload_after_change(), self.reload_downloads_page()])
    }

    pub(crate) fn remove_finished(
        &mut self,
        result: Result<Entry, remove_work::RemoveError>,
    ) -> Task<Message> {
        if !matches!(self.change, Some(Change::Remove { .. })) {
            return Task::none();
        }
        self.change = None;
        match result {
            Ok(entry) => {
                self.cancel_remove_dialog();
                self.checkups
                    .remove(&crate::checkup::Scope::Environment(entry.name.clone()));
                if self.selected.as_deref() == Some(&entry.name) {
                    self.close_panel();
                    self.selected = None;
                }
                self.toast = Some(Toast::new(entry));
            }
            Err(remove_work::RemoveError::Canceled) => self.cancel_remove_dialog(),
            Err(remove_work::RemoveError::Failed(error)) => {
                if let Some(dialog) = &mut self.removal {
                    dialog.error = Some(error);
                    // Confirm canceled the measurement, so measure again for
                    // the dialog that stays open.
                    if dialog.size.is_none()
                        && let Some(path) = dialog.prefix.clone()
                    {
                        self.remove_generation = self.remove_generation.wrapping_add(1);
                        dialog.measure = Token::new();
                        let measure = remove_work::measure(
                            self.remove_generation,
                            path,
                            dialog.measure.clone(),
                        );
                        return Task::batch([measure, self.reload_after_uses_change()]);
                    }
                }
            }
        }
        self.reload_after_uses_change()
    }

    pub(crate) fn undo_remove(&mut self) -> Task<Message> {
        if self.management_busy()
            || self.removal.is_some()
            || self.flow.is_some()
            || self.settings.is_some()
            || self.art_page.is_some()
            || self.close_prompt
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
        {
            return Task::none();
        }
        let Some(toast) = &self.toast else {
            return Task::none();
        };
        if toast.restored {
            return Task::none();
        }
        self.change_generation = self.change_generation.wrapping_add(1);
        self.change = Some(Change::Restore {
            id: toast.id.clone(),
            name: toast.name.clone(),
        });
        remove_work::restore(self.change_generation, self.paths.clone(), toast.id.clone())
    }

    pub(crate) fn undo_finished(&mut self, result: Result<Vec<String>, String>) -> Task<Message> {
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
                Ok(notes) if notes.is_empty() => self.toast = None,
                // Kept up for a moment so what the restore said isn't lost.
                Ok(notes) => {
                    toast.text = format!(
                        "Restored {}. {}",
                        toast.name,
                        notes
                            .iter()
                            .map(|note| format!("Warning: {}", wineo_core::error::one_line(note)))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    toast.restored = true;
                    toast.remaining = Duration::from_secs(10);
                    toast.last = Instant::now();
                }
                Err(error) => {
                    toast.text = error;
                    toast.remaining = Duration::from_secs(10);
                    toast.last = Instant::now();
                }
            }
        }
        self.reload_after_uses_change()
    }

    fn remove_controls(&self) -> Vec<&'static str> {
        let mut ids = Vec::new();
        let Some(dialog) = &self.removal else {
            return ids;
        };
        if self.change.is_none() && dialog.prefix.is_some() {
            ids.push(PREFIX_ID);
        }
        if !matches!(
            self.change,
            Some(Change::Remove {
                phase: Some(RemoveEvent::Moving),
                ..
            })
        ) {
            ids.push(CANCEL_ID);
        }
        if self.change.is_none() && self.remove_reason(&dialog.name).is_none() {
            ids.push(REMOVE_ID);
        }
        ids
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
                Some(Ok(bytes)) => crate::size::format_bytes(*bytes),
                Some(Err(error)) => format!("Size unavailable: {error}"),
            };
            content = content
                .push(focus_button::control(
                    checkbox(dialog.include_prefix)
                        .label("Also trash the prefix and snapshots")
                        .on_toggle_maybe(self.change.is_none().then_some(Message::RemovePrefix))
                        .into(),
                    self.change.is_none(),
                    PREFIX_ID,
                    Message::RemovePrefix(!dialog.include_prefix),
                ))
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
                    spinner::spinner(palette, 16.0),
                    text(match &self.change {
                        Some(Change::Remove {
                            phase: Some(RemoveEvent::Moving),
                            ..
                        }) => "Moving to the trash...",
                        Some(Change::Remove {
                            phase: Some(RemoveEvent::Waiting),
                            ..
                        }) => "Waiting for wine to exit",
                        _ => "Removing...",
                    })
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        let reason = self.remove_reason_with(&self.facts, &dialog.name);
        if !running && let Some(reason) = &reason {
            content = content.push(text(reason.clone()).size(13).color(palette.muted));
        }
        let mut controls = row![
            Space::new().width(Fill),
            focus_button::control(
                secondary_button("Cancel", palette)
                    .on_press_maybe((!moving).then_some(Message::CancelRemove))
                    .into(),
                !moving,
                CANCEL_ID,
                Message::CancelRemove,
            )
        ]
        .spacing(12);
        if !running {
            controls = controls.push(focus_button::control(
                accent_base("Remove", palette.danger)
                    .on_press_maybe(reason.is_none().then_some(Message::ConfirmRemove))
                    .into(),
                reason.is_none(),
                REMOVE_ID,
                Message::ConfirmRemove,
            ));
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
        let enabled = !self.management_busy()
            && !self.trash_confirm_open()
            && !self.downloads_confirm_open()
            && self.hook_stop_confirm.is_none()
            && self.flow.is_none()
            && self.removal.is_none()
            && self.settings.is_none()
            && self.art_page.is_none()
            && !self.close_prompt;
        let mut content = row![text(&toast.text).width(Fill).wrapping(TOAST_WRAPPING)]
            .spacing(12)
            .align_y(Center);
        if !toast.restored {
            content = content.push(crate::focus_button::undo(
                secondary_button("Undo", palette)
                    .on_press_maybe(enabled.then_some(Message::Undo))
                    .into(),
                enabled,
            ));
        }
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

#[cfg(test)]
mod tests;
